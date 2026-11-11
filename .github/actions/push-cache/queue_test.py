#!/usr/bin/env python3
"""Exercise queue recovery with the real store fixtures created by test.sh."""

import json
from pathlib import Path
import subprocess
import sys
import tempfile

import local


def check(drv, output, dependency, unbuilt_drv, unbuilt_output):
    with tempfile.TemporaryDirectory() as work:
        state = Path(work) / "state"
        events = Path(work) / "events"
        events.mkdir()
        (events / Path(drv).name).write_text(f"{drv}\n{output}\n")
        db = local.open_queue(state)
        local.record_completed(db, events)
        assert db.execute("SELECT outputs FROM completed WHERE drv=?", (drv,)).fetchone() == (json.dumps([output]),)
        local.register(db, f"closures old-revision {drv} {output}")
        db.close()

        db = local.open_queue(state)
        local.register(db, f"outputs current-revision {unbuilt_drv} {unbuilt_output}")
        local.discover(db, state)
        rows, selection = local.pending_selection(db)
        assert [row[2] for row in rows] == [drv]
        assert selection["PUBLIC_BUILDS"] == drv
        root = state / "roots" / rows[0][0]
        references = subprocess.check_output(["nix-store", "--query", "--requisites", str(root.resolve())], text=True).split()
        assert output in references and dependency in references
        assert not subprocess.run(["nix-store", "--check-validity", unbuilt_output], stderr=subprocess.DEVNULL).returncode == 0

        def offline(_selection):
            raise OSError("Connection unavailable")

        try:
            local.flush(db, state, offline)
        except OSError:
            pass
        else:
            raise AssertionError("Upload failure was ignored")
        db.close()
        db = local.open_queue(state)
        assert root.exists() and local.pending_selection(db)[0]

        def copy(_selection):
            subprocess.run(["nix", "copy", "--to", f"file://{work}/cache", output, dependency], check=True)

        local.flush(db, state, copy)
        assert not root.is_symlink() and not local.pending_selection(db)[0]
        db.close()
        db = local.open_queue(state)
        local.register(db, f"closures same-revision {drv} {output}")
        local.discover(db, state)
        assert not local.pending_selection(db)[0]
        assert db.execute("SELECT status FROM approvals WHERE drv=?", (unbuilt_drv,)).fetchone() == ("waiting",)
        (events / Path(unbuilt_drv).name).write_text(f"{unbuilt_drv}\n{unbuilt_output}\n")
        local.record_completed(db, events)
        try:
            local.discover(db, state)
        except ValueError as error:
            assert "collected before upload" in str(error)
        else:
            raise AssertionError("Previously completed but missing output was silently skipped")
        db.close()
    print("Queue self-check passed: revision changes, reboot recovery, retry retention, build-closure GC roots, and successful acknowledgement")


if __name__ == "__main__":
    check(*sys.argv[1:])
