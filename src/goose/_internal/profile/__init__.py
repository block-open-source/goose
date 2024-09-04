from typing import Any, Dict
from goose.profile import Profile, ToolkitSpec


def default_profile(provider: str, processor: str, accelerator: str, **kwargs: Dict[str, Any]) -> Profile:
    """Get the default profile"""

    # TODO consider if the providers should have recommended models

    return Profile(
        provider=provider,
        processor=processor,
        accelerator=accelerator,
        moderator="truncate",
        toolkits=[ToolkitSpec("developer")],
    )
