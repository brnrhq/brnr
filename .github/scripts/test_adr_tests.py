"""The inventory must fail closed without mistaking documentation for tests."""

import contextlib
import importlib.util
import io
from pathlib import Path
import tempfile
import sys
import unittest

sys.dont_write_bytecode = True

spec = importlib.util.spec_from_file_location("adr_tests", Path(__file__).with_name("adr-tests.py"))
adr_tests = importlib.util.module_from_spec(spec)
spec.loader.exec_module(adr_tests)


class InventoryTests(unittest.TestCase):
    def check(self, record, listing, ignored=""):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "docs/adr").mkdir(parents=True)
            (root / "docs/adr/0007-start.md").write_text(record)
            output = io.StringIO()
            with contextlib.redirect_stdout(output):
                ok = adr_tests.report(root, listing, ignored)
            return ok, output.getvalue()

    def test_missing_registered_claim_fails_even_with_a_test_name_in_the_adr(self):
        ok, output = self.check(
            "# 7. Start\n\nImplemented.\n\n## Tests\nadr_0007_atomic",
            "unrelated: test\nadr_0007_atomic: benchmark\n",
        )
        self.assertFalse(ok)
        self.assertIn("MISSING TESTS", output)

    def test_registered_unit_and_integration_claims_count(self):
        ok, output = self.check(
            "# 7. Start\n\nImplemented.\n",
            "start::tests::adr_0007_atomic: test\nadr_0007_abandoned: test\n",
        )
        self.assertTrue(ok)
        self.assertIn("2 registered claim tests", output)

    def test_an_ignored_claim_does_not_count(self):
        listing = "adr_0007_atomic: test\n"
        ok, output = self.check("# 7. Start\n\nImplemented.\n", listing, listing)
        self.assertFalse(ok)
        self.assertIn("Ignored ADR test is not evidence", output)

    def test_unknown_or_malformed_adr_fails(self):
        for name in ["adr_9999_unknown", "adr_7_atomic", "adr_0007_"]:
            with self.subTest(name=name):
                ok, _ = self.check("# 7. Start\n", f"{name}: test\n")
                self.assertFalse(ok)

    def test_exemption_is_reported_without_claiming_coverage(self):
        record = "# 7. Start\n\nImplemented.\n\nTest exemption: Requires an external service.\n"
        ok, output = self.check(record, "")
        self.assertTrue(ok)
        self.assertIn("NO RUST CLAIM TEST", output)
        ok, _ = self.check(record, "adr_0007_atomic: test\n")
        self.assertFalse(ok, "a newly tested ADR must remove its stale exemption")


if __name__ == "__main__":
    unittest.main()
