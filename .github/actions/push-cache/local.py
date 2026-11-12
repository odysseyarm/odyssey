#!/usr/bin/env python3
"""Persist exact approvals and retain built outputs until their upload succeeds."""

import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import sqlite3
import subprocess
import tempfile
import sys

SCRIPTS = Path(__file__).resolve().parent
STORE_PATH = re.compile(r"/nix/store/[0-9abcdfghijklmnpqrsvwxyz]{32}-[^/\s]+\Z")


def store_path(value):
    if not STORE_PATH.fullmatch(value):
        raise ValueError(f"Invalid store path: {value!r}")
    return value


def open_queue(state):
    state.mkdir(mode=0o700, parents=True, exist_ok=True)
    db = sqlite3.connect(state / "queue.sqlite")
    db.execute("PRAGMA synchronous = FULL")
    db.execute("""CREATE TABLE IF NOT EXISTS approvals (
        id TEXT PRIMARY KEY, policy TEXT, drv TEXT, outputs TEXT,
        private_sources TEXT, status TEXT NOT NULL DEFAULT 'waiting')""")
    db.execute("CREATE TABLE IF NOT EXISTS completed (drv TEXT PRIMARY KEY, outputs TEXT)")
    return db


def register(db, plan):
    rows = []
    private = []
    for line in plan.splitlines():
        fields = line.split()
        if not fields:
            continue
        if fields[0] == "private-source" and len(fields) == 2:
            private.append(store_path(fields[1]))
            continue
        if len(fields) < 4 or fields[0] not in ("closures", "outputs"):
            raise ValueError(f"Invalid approval: {line!r}")
        policy, _name, drv, *outputs = fields
        store_path(drv)
        if not drv.endswith(".drv"):
            raise ValueError(f"Approval is not a derivation: {drv}")
        rows.append((policy, drv, sorted({store_path(out) for out in outputs})))
    with db:
        for policy, drv, outputs in rows:
            values = policy, drv, json.dumps(outputs), json.dumps(sorted(set(private)))
            key = hashlib.sha256(json.dumps(values).encode()).hexdigest()
            db.execute("INSERT OR IGNORE INTO approvals (id,policy,drv,outputs,private_sources) VALUES (?,?,?,?,?)", (key, *values))


def record_completed(db, events):
    if not events.exists():
        return
    with db:
        for event in events.iterdir():
            if event.name.startswith("."):
                continue
            fields = event.read_text().split()
            if len(fields) < 2:
                raise ValueError(f"Incomplete build event: {event}")
            drv, *outputs = map(store_path, fields)
            if not drv.endswith(".drv") or event.name != Path(drv).name:
                raise ValueError(f"Invalid build event: {event}")
            db.execute("INSERT OR REPLACE INTO completed VALUES (?,?)", (drv, json.dumps(sorted(set(outputs)))))


def discover(db, state):
    roots = state / "roots"
    roots.mkdir(exist_ok=True)
    for key, policy, drv, encoded, recorded in db.execute("""SELECT a.id,a.policy,a.drv,a.outputs,c.outputs
            FROM approvals a LEFT JOIN completed c ON a.drv=c.drv
            WHERE a.status='waiting'""").fetchall():
        outputs = json.loads(encoded)
        if recorded is not None and not set(outputs).issubset(json.loads(recorded)):
            raise ValueError(f"Build outputs differ from approval: {drv}")
        if subprocess.run(["nix-store", "--check-validity", *outputs], stderr=subprocess.DEVNULL).returncode:
            if recorded is not None:
                raise ValueError(f"Completed approved output was collected before upload; rebuild {drv}")
            continue
        references = outputs
        if policy == "closures":
            references = subprocess.check_output(["nix-store", "--query", "--requisites", "--include-outputs", drv], text=True).split()
        with tempfile.NamedTemporaryFile(mode="w", dir=state) as reference_file:
            json.dump(references, reference_file)
            reference_file.flush()
            expression = 'builtins.toFile "odyssey-cache-pending" (builtins.concatStringsSep "\\n" (map builtins.storePath (builtins.fromJSON (builtins.readFile ' + json.dumps(reference_file.name) + '))))'
            manifest = subprocess.check_output(["nix", "eval", "--impure", "--raw", "--expr", expression], text=True).strip()
        subprocess.run(["nix-store", "--add-root", str(roots / key), "--indirect", "--realise", manifest], check=True, stdout=subprocess.DEVNULL)
        with db:
            db.execute("UPDATE approvals SET status='pending' WHERE id=?", (key,))


def pending_selection(db):
    rows = db.execute("SELECT id,policy,drv,outputs,private_sources FROM approvals WHERE status='pending' ORDER BY id").fetchall()
    outputs, closures, private = set(), set(), set()
    for _key, policy, drv, encoded, sources in rows:
        if policy == "closures":
            closures.add(drv)
        else:
            outputs.update(json.loads(encoded))
        private.update(json.loads(sources))
    return rows, {
        "PATHS": " ".join(sorted(outputs)),
        "PUBLIC_BUILDS": " ".join(sorted(closures)),
        "PRIVATE_SOURCES": " ".join(sorted(private)),
    }

def flush(db, state, upload):
    rows, selection = pending_selection(db)
    if rows:
        upload(selection)
        with db:
            db.executemany("UPDATE approvals SET status='uploaded' WHERE id=?", [(row[0],) for row in rows])
    for (key,) in db.execute("SELECT id FROM approvals WHERE status='uploaded'"):
        (state / "roots" / key).unlink(missing_ok=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--register", metavar="FLAKE", help="persist approvals without uploading or building")
    mode.add_argument("--flush", action="store_true", help="flush saved approvals without evaluating working checkout")
    mode.add_argument("--dry-run", action="store_true", help="print approved paths without signing or uploading")
    args = parser.parse_args()
    state = Path(os.environ.get("ODYSSEY_CACHE_STATE", Path(os.environ.get("XDG_STATE_HOME", Path.home() / ".local/state")) / "odyssey-cache")).absolute()
    state.mkdir(mode=0o700, parents=True, exist_ok=True)
    with (state / "lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        with open_queue(state) as db:
            if not args.flush:
                plan = os.environ.get("PUBLIC_CACHE_PLAN")
                if plan is None:
                    plan = subprocess.check_output(["bash", str(SCRIPTS / "register.sh"), args.register or "."], text=True)
                register(db, plan)
            if args.register:
                return
            record_completed(db, Path(os.environ.get("ODYSSEY_CACHE_EVENTS", "/var/lib/nix-post-build/events")))
            discover(db, state)
            rows, selection = pending_selection(db)
            if not rows:
                print("Nothing approved is pending upload", file=sys.stderr)
            if args.dry_run and rows:
                subprocess.run(["bash", str(SCRIPTS / "paths.sh")], env=os.environ | selection, check=True)
            if args.dry_run:
                return
            flush(db, state, lambda env: subprocess.run(["bash", str(SCRIPTS / "upload.sh")], env=os.environ | env, check=True))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, sqlite3.Error, subprocess.CalledProcessError) as error:
        print(f"Cache upload failed; pending outputs retained: {error}", file=sys.stderr)
        sys.exit(1)
