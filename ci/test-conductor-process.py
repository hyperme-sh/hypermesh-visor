"""Unit tests for shutdown observation; never start a service."""
import importlib.util
from pathlib import Path
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("recipe", Path(__file__).with_name("check-conductor-recipe.py"))
recipe = importlib.util.module_from_spec(spec)
spec.loader.exec_module(recipe)


class ProcessExitTests(unittest.TestCase):
    def test_permission_denied_is_not_exit(self):
        with patch.object(recipe, "process_identity", side_effect=PermissionError()):
            self.assertFalse(recipe.process_exited("123", "456"))

    def test_running_or_stopped_same_process_is_not_exit(self):
        for state in ("R", "S", "D", "T"):
            with self.subTest(state=state), patch.object(recipe, "process_identity", return_value=(state, "456")):
                self.assertFalse(recipe.process_exited("123", "456"))

    def test_zombie_missing_and_reused_pid_are_exit(self):
        for state, started in (("Z", "456"), ("X", "456"), ("S", "999")):
            with self.subTest(state=state), patch.object(recipe, "process_identity", return_value=(state, started)):
                self.assertTrue(recipe.process_exited("123", "456"))
        with patch.object(recipe, "process_identity", side_effect=FileNotFoundError()):
            self.assertTrue(recipe.process_exited("123", "456"))

    def test_stat_parser_handles_spaces_and_parentheses_in_comm(self):
        fields = ["S"] + ["0"] * 18 + ["456"] + ["0"] * 10
        with patch.object(Path, "read_text", return_value="123 (worker (special)) " + " ".join(fields)):
            self.assertEqual(recipe.process_identity("123"), ("S", "456"))


if __name__ == "__main__":
    unittest.main()
