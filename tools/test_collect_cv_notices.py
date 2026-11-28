import importlib.util
from pathlib import Path
import unittest


spec = importlib.util.spec_from_file_location(
    "collect_cv_notices", Path(__file__).with_name("collect-cv-notices.py")
)
assert spec is not None and spec.loader is not None
notices = importlib.util.module_from_spec(spec)
spec.loader.exec_module(notices)


class LicenseSelectionTests(unittest.TestCase):
    def test_unicode_requirement_survives_mit_choice(self) -> None:
        self.assertEqual(
            notices.selected_license("(MIT OR Apache-2.0) AND Unicode-3.0"),
            "MIT AND Unicode-3.0",
        )

    def test_legacy_dual_license_spelling_selects_mit(self) -> None:
        self.assertEqual(notices.selected_license("MIT / Apache-2.0"), "MIT")

    def test_unreviewed_terms_are_not_silently_dropped(self) -> None:
        for expression in (
            "MIT AND BSD-3-Clause",
            "MIT-0",
            "Apache-2.0 WITH custom-exception",
            "",
        ):
            with self.subTest(expression=expression), self.assertRaises(ValueError):
                notices.selected_license(expression)

    def test_mpl_source_requirement_is_not_replaced_by_mit(self) -> None:
        self.assertEqual(notices.selected_license("MPL-2.0"), "MPL-2.0")


if __name__ == "__main__":
    unittest.main()
