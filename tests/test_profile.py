from goose.profile import ToolkitSpec


def test_profile_info(profile_factory):
    profile = profile_factory(
        {
            "provider": "provider",
            "processor": "processor",
            "toolkits": [ToolkitSpec("developer"), ToolkitSpec("github")],
        }
    )
    assert profile.profile_info() == "provider:provider, processor:processor toolkits: developer, github"
