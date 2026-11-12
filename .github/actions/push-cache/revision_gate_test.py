#!/usr/bin/env python3
"""Check multi-revision uploads and push blocking against a real local jj remote."""

import os
from pathlib import Path
import subprocess
import sys
import tempfile

import local

SCRIPTS = Path(__file__).resolve().parent


def check(shell, private):
    with tempfile.TemporaryDirectory() as work:
        root = Path(work)
        repo = root / "repo"
        remote = root / "remote"
        state = root / "queue"
        jj_config = root / "jj.toml"
        jj_config.write_text('[user]\nname = "Cache Test"\nemail = "cache-test@example.invalid"\n')
        env = os.environ | {"JJ_CONFIG": str(jj_config), "ODYSSEY_CACHE_STATE": str(state), "ODYSSEY_CACHE_EVENTS": str(root / "events"), "NIX_SIGNING_KEY_FILE": str(root / "missing-key")}

        def run(*args, check=True):
            result = subprocess.run(args, cwd=repo, env=env, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            if check and result.returncode:
                raise RuntimeError(f"{args}: {result.stdout}\n{result.stderr}")
            return result

        subprocess.run(["jj", "git", "init", "--colocate", str(remote)], env=env, check=True)
        subprocess.run(["jj", "git", "init", "--colocate", str(repo)], env=env, check=True)
        run("jj", "git", "remote", "add", "origin", str(remote))
        system = run("nix", "eval", "--impure", "--raw", "--expr", "builtins.currentSystem").stdout

        def write_flake(name, policy):
            (repo / "flake.nix").write_text(f'''{{
  inputs.odyssey-cv = {{ url = "path:{private}"; flake = false; }};
  outputs = inputs: let drv = builtins.derivation {{
    name = "{name}"; system = "{system}"; builder = builtins.storePath "{shell}";
    outputs = [ "out" ];
    args = [ "-c" "printf fixture > $out" ];
  }}; in {{
    packages.{system}.xtask = drv;
    {f'legacyPackages.{system}.publicCache = {{ closures.xtask = drv; outputs = {{}}; }};' if policy else ''}
  }};
}}''')

        write_flake("cache-revision-first", False)
        run("jj", "status")
        run("nix", "flake", "lock", "--offline")
        run("jj", "describe", "-m", "test: First cache revision")
        first = run("jj", "log", "--no-graph", "-r", "@", "-T", "commit_id").stdout
        run("nix", "build", "--impure", "--no-link", f"git+file://{repo}?rev={first}#xtask")
        run("jj", "new", "-m", "test: Second cache revision")
        write_flake("cache-revision-second", True)
        run("jj", "describe", "-m", "test: Second cache revision")
        second = run("jj", "log", "--no-graph", "-r", "@", "-T", "commit_id").stdout
        run("nix", "build", "--impure", "--no-link", f"git+file://{repo}?rev={second}#xtask")
        run("jj", "bookmark", "create", "cache-test")

        dry = run("bash", str(SCRIPTS / "push.sh"), "--bookmark", "cache-test", "--dry-run")
        assert not (state / "queue.sqlite").exists(), dry.stdout + dry.stderr
        failed = run("bash", str(SCRIPTS / "push.sh"), "--bookmark", "cache-test", check=False)
        assert failed.returncode != 0
        assert not run("jj", "log", "--no-graph", "-r", "remote_bookmarks()", "-T", "commit_id").stdout
        assert (state / "queue.sqlite").exists(), failed.stdout + failed.stderr
        db = local.open_queue(state)
        rows, _selection = local.pending_selection(db)
        assert len(rows) == 2, failed.stdout + failed.stderr

        def copy(selection):
            selected = subprocess.check_output(["bash", str(SCRIPTS / "paths.sh")], cwd=repo, env=env | selection, text=True).split()
            run("nix", "copy", "--to", f"file://{root}/cache", *selected)

        local.flush(db, state, copy)
        db.close()
        pushed = run("bash", str(SCRIPTS / "push.sh"), "--bookmark", "cache-test")
        assert run("jj", "log", "--no-graph", "-r", "remote_bookmarks()", "-T", "commit_id").stdout == second, pushed.stdout + pushed.stderr
        run("bash", str(SCRIPTS / "push.sh"), "--bookmark", "cache-test")
        run("jj", "bookmark", "create", "cache-ancestor", "-r", first)
        env["ODYSSEY_CACHE_STATE"] = str(root / "ancestor-queue")
        failed = run("bash", str(SCRIPTS / "push.sh"), "--bookmark", "cache-ancestor", check=False)
        assert failed.returncode != 0
        db = local.open_queue(root / "ancestor-queue")
        assert len(local.pending_selection(db)[0]) == 1, failed.stdout + failed.stderr
        db.close()
        assert run("jj", "log", "--no-graph", "-r", "remote_bookmarks()", "-T", "commit_id").stdout == second
        env["ODYSSEY_CACHE_STATE"] = str(state)
        run("jj", "bookmark", "delete", "cache-ancestor")
        run("jj", "bookmark", "delete", "cache-test")
        deleted = run("bash", str(SCRIPTS / "push.sh"), "--deleted")
        assert not run("jj", "log", "--no-graph", "-r", "remote_bookmarks()", "-T", "commit_id").stdout, deleted.stdout + deleted.stderr
    print("Revision gate self-check passed: legacy and current revisions, dry run, failed upload blocks push, successful upload permits push, no-op and deletion")


if __name__ == "__main__":
    check(*sys.argv[1:])
