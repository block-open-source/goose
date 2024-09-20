from goose.toolkit.utils import parse_plan


def test_parse_plan_simple():
    plan_str = (
        "Here is python repo\n"
        "-use uv\n"
        "-do not use poetry\n\n"
        "Now you should:\n\n"
        "-Open a file\n"
        "-Run a test"
    )
    expected_result = {
        "kickoff_message": "Here is python repo\n-use uv\n-do not use poetry\n\nNow you should:",
        "tasks": ["Open a file", "Run a test"],
    }
    assert expected_result == parse_plan(plan_str)


def test_parse_plan_multiple_groups():
    plan_str = (
        "Here is python repo\n"
        "-use uv\n"
        "-do not use poetry\n\n"
        "Now you should:\n\n"
        "-Open a file\n"
        "-Run a test\n\n"
        "Now actually follow the steps:\n"
        "-Step1\n"
        "-Step2"
    )
    expected_result = {
        "kickoff_message": (
            "Here is python repo\n"
            "-use uv\n"
            "-do not use poetry\n\n"
            "Now you should:\n\n"
            "-Open a file\n"
            "-Run a test\n\n"
            "Now actually follow the steps:"
        ),
        "tasks": ["Step1", "Step2"],
    }
    assert expected_result == parse_plan(plan_str)


def test_parse_plan_empty_tasks():
    plan_str = "Here is python repo"
    expected_result = {"kickoff_message": "Here is python repo", "tasks": []}
    assert expected_result == parse_plan(plan_str)


def test_parse_plan_empty_kickoff_message():
    plan_str = "-task1\n-task2"
    expected_result = {"kickoff_message": "", "tasks": ["task1", "task2"]}
    assert expected_result == parse_plan(plan_str)


def test_parse_plan_with_numbers():
    plan_str = "Here is python repo\n" "Now you should:\n\n" "-1 Open a file\n" "-2 Run a test"
    expected_result = {
        "kickoff_message": "Here is python repo\nNow you should:",
        "tasks": ["1 Open a file", "2 Run a test"],
    }
    assert expected_result == parse_plan(plan_str)
