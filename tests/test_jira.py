import unittest
from unittest.mock import patch
from goose.toolkit.jira import Jira


class TestJiraToolkit(unittest.TestCase):
    @patch("goose.toolkit.jira.Message.load")
    def test_jira_system_prompt(self, mock_load):
        mock_load.return_value.text = "This is a prompt for jira"
        jira_toolkit = Jira(None)
        prompt = jira_toolkit.system()
        # Ensure Jinja template syntax isn't present in the loaded prompt
        self.assertNotIn("{%", prompt)
        self.assertNotIn("%}", prompt)
        self.assertEqual(prompt, "This is a prompt for jira")

    def test_is_jira_issue(self):
        jira_toolkit = Jira(None)
        valid_jira_issue = "PROJ-123"
        invalid_jira_issue = "INVALID_ISSUE"
        # Ensure the regex correctly identifies valid JIRA issues
        self.assertTrue(jira_toolkit.is_jira_issue(valid_jira_issue))
        self.assertFalse(jira_toolkit.is_jira_issue(invalid_jira_issue))


if __name__ == "__main__":
    unittest.main()
