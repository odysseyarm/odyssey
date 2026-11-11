import tempfile
import unittest
from pathlib import Path

import impact

MANIFESTS = {
    "Cargo.toml": """
[workspace]
members = ["crates/*"]
exclude = ["sdk/odyssey-hub-client-ffi", "sdk/odyssey-hub-client-uniffi", "crates/xtask"]
[workspace.dependencies]
odyssey-protocol = { path = "crates/odyssey-protocol" }
""",
    "crates/odyssey-protocol/Cargo.toml": "[package]\nname='p'\n",
    "crates/odyssey-version/Cargo.toml": "[package]\nname='v'\n",
    "crates/odyssey-sim/Cargo.toml": "[package]\nname='s'\n",
    "crates/odyssey-hub-server/Cargo.toml": """
[package]
name='h'
[dependencies]
odyssey-protocol = { workspace = true }
""",
    "crates/xtask/Cargo.toml": "[package]\nname='x'\n",
    "sdk/odyssey-hub-client-ffi/Cargo.toml": """
[package]
name='f'
[target.'cfg(windows)'.dependencies.odyssey-protocol]
path = "../../crates/odyssey-protocol"
""",
    "sdk/odyssey-hub-client-uniffi/Cargo.toml": "[package]\nname='u'\n[workspace]\n",
}


def select(*changed):
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        for name, text in MANIFESTS.items():
            (root / name).parent.mkdir(parents=True, exist_ok=True)
            (root / name).write_text(text)
        graph, workspace = impact.load_graph(root, list(MANIFESTS))
        result = impact.affected(list(changed), impact.build_rules(graph, workspace))
    return {suite for suite, reason in result.items() if reason}


class SelectionTests(unittest.TestCase):
    def test_documentation_affects_nothing(self):
        self.assertEqual(select("docs/index.md", "README.md"), set())

    def test_a_shared_crate_reaches_the_sdk_that_depends_on_it(self):
        affected = select("crates/odyssey-protocol/src/lib.txt")
        self.assertTrue({"host", "ffi", "proofs", "tracking"} <= affected)
        self.assertFalse({"uniffi", "xtask"} & affected)

    def test_an_sdk_change_stays_inside_that_sdk(self):
        self.assertEqual(select("sdk/odyssey-hub-client-uniffi/uniffi.toml"), {"uniffi"})

    def test_samples_are_not_built(self):
        self.assertEqual(select("sdk/odyssey-hub-client-ffi/samples/a.c"), set())

    def test_a_manifest_with_a_byte_order_mark_is_read(self):
        with tempfile.TemporaryDirectory() as directory:
            (Path(directory) / "Cargo.toml").write_bytes(b"\xef\xbb\xbf[workspace]\n")
            graph, workspace = impact.load_graph(Path(directory), ["Cargo.toml"])
        self.assertEqual((graph, workspace), ({}, {}))

    def test_the_sdk_ignores_the_root_lock_file(self):
        affected = select("Cargo.lock")
        self.assertTrue({"host", "tracking", "proofs", "xtask"} <= affected)
        self.assertFalse({"ffi", "uniffi"} & affected)

    def test_rust_sources_are_always_formatted(self):
        self.assertEqual(select("firmware/x/src/main.rs"), {"format", "xtask"})

    def test_the_toolchain_reaches_every_suite(self):
        self.assertEqual(select("rust-toolchain.toml"), set(impact.SUITES))

    def test_a_workflow_runs_only_its_own_suite(self):
        self.assertEqual(select(".github/workflows/fmt.yml"), {"format"})

    def test_a_malformed_manifest_fails_selection(self):
        with tempfile.TemporaryDirectory() as directory:
            (Path(directory) / "Cargo.toml").write_text("[workspace\n")
            with self.assertRaises(Exception):
                impact.load_graph(Path(directory), ["Cargo.toml"])

    def test_events_without_a_base_run_everything(self):
        for event in ("push", "schedule", "workflow_dispatch"):
            environment = {"EVENT": event, "REF_NAME": "master", "DEFAULT_BRANCH": "master"}
            self.assertIsNone(impact.comparison(environment), event)
        branch = {"EVENT": "workflow_dispatch", "REF_NAME": "topic", "DEFAULT_BRANCH": "master"}
        self.assertEqual(impact.comparison(branch), ("origin/master", "HEAD"))


if __name__ == "__main__":
    unittest.main()
