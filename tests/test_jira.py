import pytest
from goose.toolkit.jira import Jira


@pytest.fixture
def jira_toolkit():
    return Jira(None)


def test_jira_system_prompt(jira_toolkit):
    prompt = jira_toolkit.system()
    print("System Prompt:\n", prompt)
    # Ensure Jinja template syntax isn't present in the loaded prompt
    # Ensure both installation instructions are present in the prompt
    assert "macos" in prompt
    assert "On other operating systems or for alternative installation methods" in prompt


def test_is_jira_issue(jira_toolkit):
    valid_jira_issue = "PROJ-123"
    invalid_jira_issue = "INVALID_ISSUE"
    # Ensure the regex correctly identifies valid JIRA issues
    assert jira_toolkit.is_jira_issue(valid_jira_issue)
    assert not jira_toolkit.is_jira_issue(invalid_jira_issue)
