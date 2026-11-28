#!/usr/bin/env python3
"""Check multi-revision uploads and push blocking against a real local jj remote."""

import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

import local

SCRIPTS = Path(__file__).resolve().parent


def check(shell: str, private: str) -> None:
    with tempfile.TemporaryDirectory() as work:
        root = Path(work)
        repo = root / "repo"
        remote = root / "remote"
        state = root / "queue"
        jj_config = root / "jj.toml"
        jj_config.write_text(
            '[user]\nname = "Cache Test"\nemail = "cache-test@example.invalid"\n'
            "[aliases]\npush = "
            + json.dumps(
                ["util", "exec", "--", "bash", str(SCRIPTS / "local.sh"), "--push"]
            )
            + '\n[remotes.origin]\nauto-track-bookmarks = "*"\n'
        )
        env = os.environ | {
            "JJ_CONFIG": str(jj_config),
            "ODYSSEY_CACHE_STATE": str(state),
            "ODYSSEY_CACHE_EVENTS": str(root / "events"),
            "NIX_SIGNING_KEY_FILE": str(root / "missing-key"),
        }

        def run(
            *args: str, check: bool = True, cwd: Path = repo
        ) -> subprocess.CompletedProcess[str]:
            result = subprocess.run(
                args,
                cwd=cwd,
                env=env,
                text=True,
                capture_output=True,
                check=False,
            )
            if check and result.returncode:
                raise RuntimeError(f"{args}: {result.stdout}\n{result.stderr}")
            return result

        subprocess.run(
            ["jj", "git", "init", "--colocate", str(remote)], env=env, check=True
        )
        subprocess.run(
            ["jj", "git", "init", "--colocate", str(repo)], env=env, check=True
        )
        run("jj", "git", "remote", "add", "origin", str(remote))
        system = run(
            "nix", "eval", "--impure", "--raw", "--expr", "builtins.currentSystem"
        ).stdout

        def write_flake(name: str, policy: bool, cwd: Path = repo) -> None:
            name = f"{name}-{root.name}"
            (cwd / "flake.nix").write_text(f'''{{
  inputs.odyssey-cv = {{ url = "path:{private}"; flake = false; }};
  outputs = inputs: let
    builder = builtins.appendContext "{shell}" {{ "{Path(shell).parent.parent}" = {{ path = true; }}; }};
    drv = builtins.derivation {{
      name = "{name}"; system = "{system}"; inherit builder;
      outputs = [ "out" ];
      args = [ "-c" "printf fixture > $out" ];
    }};
    artifacts = builtins.derivation {{
      name = "{name}-deps"; system = "{system}"; inherit builder;
      outputs = [ "out" ];
      args = [ "-c" "printf dependency > $out" ];
    }};
    lint = builtins.derivation {{
      name = "{name}-lint"; system = "{system}"; inherit builder;
      outputs = [ "out" ];
      args = [ "-c" "printf lint > $out" ];
    }};
  in {{
    packages.{system}.xtask = drv;
    packages.{system}.host = drv // {{ cargoArtifacts = artifacts; }};
    checks.{system}.host-lint = lint;
    checks.{system}.host-lint-bins = lint;
    {f"legacyPackages.{system}.publicCache = {{ closures = {{ xtask = drv; host-artifacts = artifacts; }}; outputs = {{}}; }};" if policy else ""}
  }};
}}''')

        write_flake("cache-revision-first", False)
        run("jj", "status")
        run("nix", "flake", "lock", "--offline")
        run("jj", "describe", "-m", "test: First cache revision")
        first = run("jj", "log", "--no-graph", "-r", "@", "-T", "commit_id").stdout
        run(
            "nix",
            "build",
            "--impure",
            "--no-link",
            f"git+file://{repo}?rev={first}#xtask",
        )
        run("jj", "new", "-m", "test: Second cache revision")
        write_flake("cache-revision-second", True)
        run("jj", "describe", "-m", "test: Second cache revision")
        second = run("jj", "log", "--no-graph", "-r", "@", "-T", "commit_id").stdout
        run("jj", "bookmark", "create", "cache-test")
        for label, command in (
            ("dependency", "printf dependency > $out"),
            ("lint", "printf lint > $out"),
        ):
            run("jj", "new", "-r", second, "-m", f"test: Failing {label} revision")
            write_flake(f"cache-revision-{label}-failure", True)
            flake = repo / "flake.nix"
            flake.write_text(flake.read_text().replace(command, "exit 17"))
            run("jj", "bookmark", "create", "build-failure")
            rejected = run(
                "jj",
                "push",
                "--bookmark",
                "build-failure",
                check=False,
            )
            assert rejected.returncode != 0, rejected.stdout + rejected.stderr
            assert not (state / "queue.sqlite").exists()
            assert not run(
                "jj", "log", "--no-graph", "-r", "remote_bookmarks()", "-T", "commit_id"
            ).stdout
            run("jj", "bookmark", "delete", "build-failure")
        run("jj", "new", "-r", second, "-m", "test: Push checked revision")

        dry = run("jj", "push", "--bookmark", "cache-test", "--dry-run")
        assert not (state / "queue.sqlite").exists(), dry.stdout + dry.stderr
        failed = run("jj", "push", "--bookmark", "cache-test", check=False)
        assert failed.returncode != 0
        assert not run(
            "jj", "log", "--no-graph", "-r", "remote_bookmarks()", "-T", "commit_id"
        ).stdout
        assert (state / "queue.sqlite").exists(), failed.stdout + failed.stderr
        db = local.open_queue(state)
        rows, _selection = local.pending_selection(db)
        assert len(rows) == 3, failed.stdout + failed.stderr
        dependencies = run(
            "nix",
            "eval",
            "--raw",
            f"git+file://{repo}?rev={second}#host.cargoArtifacts.drvPath",
        ).stdout
        assert dependencies in _selection["PUBLIC_BUILDS"].split(), _selection

        def copy(selection: dict[str, str]) -> None:
            selected = subprocess.check_output(
                ["bash", str(SCRIPTS / "paths.sh")],
                cwd=repo,
                env=env | selection,
                text=True,
            ).split()
            run("nix", "copy", "--to", f"file://{root}/cache", *selected)

        local.flush(db, state, copy)
        db.close()
        pushed = run("jj", "push", "--bookmark", "cache-test")
        assert (
            run(
                "jj", "log", "--no-graph", "-r", "remote_bookmarks()", "-T", "commit_id"
            ).stdout
            == second
        ), pushed.stdout + pushed.stderr
        run("jj", "push", "--bookmark", "cache-test")
        run("jj", "bookmark", "create", "cache-ancestor", "-r", first)
        env["ODYSSEY_CACHE_STATE"] = str(root / "ancestor-queue")
        failed = run(
            "jj",
            "push",
            "--bookmark",
            "cache-ancestor",
            check=False,
        )
        assert failed.returncode != 0
        db = local.open_queue(root / "ancestor-queue")
        assert len(local.pending_selection(db)[0]) == 2, failed.stdout + failed.stderr
        db.close()
        assert (
            run(
                "jj", "log", "--no-graph", "-r", "remote_bookmarks()", "-T", "commit_id"
            ).stdout
            == second
        )
        env["ODYSSEY_CACHE_STATE"] = str(state)
        db = local.open_queue(state)
        local.discover(db, state)
        local.flush(db, state, copy)
        db.close()
        run("jj", "bookmark", "delete", "cache-ancestor")
        run("jj", "bookmark", "delete", "cache-test")
        deleted = run("jj", "push", "--deleted")
        assert not run(
            "jj", "log", "--no-graph", "-r", "remote_bookmarks()", "-T", "commit_id"
        ).stdout, deleted.stdout + deleted.stderr
        linked = root / "linked"
        default_revision = run(
            "jj", "log", "--no-graph", "-r", "@", "-T", "commit_id"
        ).stdout
        run("jj", "workspace", "list")
        run(
            "jj",
            "workspace",
            "add",
            "--sparse-patterns",
            "empty",
            "--name",
            "linked",
            "-r",
            second,
            str(linked),
        )
        run(
            "jj",
            "sparse",
            "set",
            "--clear",
            "--add",
            "flake.nix",
            "--add",
            "flake.lock",
            cwd=linked,
        )
        write_flake("cache-revision-linked", True, cwd=linked)
        run("jj", "describe", "-m", "test: Push linked workspace", cwd=linked)
        run("jj", "bookmark", "create", "linked-cache", cwd=linked)
        linked_revision = run(
            "jj", "log", "--no-graph", "-r", "@", "-T", "commit_id", cwd=linked
        ).stdout
        assert not (linked / ".git").exists()
        preview = run("jj", "push", "--revisions", "@", "--dry-run", cwd=linked)
        assert "bookmark: linked-cache" in preview.stdout + preview.stderr, (
            preview.stdout + preview.stderr
        )
        rejected = run("jj", "push", "--revisions", "@", cwd=linked, check=False)
        assert rejected.returncode != 0, rejected.stdout + rejected.stderr
        git_repo = run("jj", "git", "root", cwd=linked).stdout.strip()
        dependencies = run(
            "nix",
            "eval",
            "--raw",
            f"git+file://{git_repo}?rev={linked_revision}#host.cargoArtifacts.drvPath",
        ).stdout
        db = local.open_queue(state)
        assert dependencies in local.pending_selection(db)[1]["PUBLIC_BUILDS"].split()
        local.flush(db, state, copy)
        db.close()
        run("jj", "push", "--revisions", "@", cwd=linked)
        assert (
            run(
                "jj",
                "log",
                "--no-graph",
                "-r",
                "linked-cache@origin",
                "-T",
                "commit_id",
            ).stdout
            == linked_revision
        )
        assert (
            run("jj", "log", "--no-graph", "-r", "@", "-T", "commit_id").stdout
            == default_revision
        )
    print(
        "Revision gate self-check passed: configured jj push alias, linked-workspace selection and shared Git store, exact-revision dependencies, build and lint failures, legacy and current revisions, dry run, failed upload blocks push, successful upload permits push, no-op and deletion"
    )


if __name__ == "__main__":
    check(*sys.argv[1:])
