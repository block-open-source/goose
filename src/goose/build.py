from itertools import chain

from exchange import Exchange, Message
from exchange.moderators import get_moderator
from exchange.providers import get_provider

from goose.notifier import Notifier
from goose.profile import Profile
from goose.toolkit import get_toolkit
from goose.toolkit.base import Requirements
from goose.view import ExchangeView


def build_exchange(profile: Profile, notifier: Notifier) -> Exchange:
    """Build an exchange configured through the profile

    This will setup any toolkits and use that to build the exchange's collection
    of tools.

    Args:
        profile (Profile): The profile specifying how to setup this exchange
        notifier (Notifier): A notifier instance used by tools to send info
    """

    provider = get_provider(profile.provider).from_env()

    # Support instantating toolkits in *two* passes for now, no further nesting
    concrete_toolkits = {}

    # First instantiate all toolkits that are sub dependencies
    for spec in profile.toolkits:
        for required in spec.requires.values():
            concrete_toolkits[required] = get_toolkit(required)(notifier=notifier, requires=Requirements(required))

    # Now that we have the dependencies available, we can instantiate everything else
    toolkits = []
    for spec in profile.toolkits:
        if spec.name in concrete_toolkits:
            toolkits.append(concrete_toolkits[spec.name])
            continue

        requires = Requirements(
            spec.name,
            {key: concrete_toolkits[val] for key, val in spec.requires.items()},
        )
        toolkit = get_toolkit(spec.name)(notifier=notifier, requires=requires)
        toolkits.append(toolkit)

    # From the toolkits, we derive the exchange prompt and tools
    system = "\n\n".join([Message.load("system.jinja").text] + [toolkit.system() for toolkit in toolkits])
    tools = tuple(chain(*(toolkit.tools() for toolkit in toolkits)))
    exchange = Exchange(
        provider=provider,
        system=system,
        tools=tools,
        moderator=get_moderator(profile.moderator)(),
        model=profile.processor,
    )

    # This is a bit awkward, but we have to set this after the fact because building
    # the exchange requires having the toolkits
    for toolkit in toolkits:
        toolkit.exchange_view = ExchangeView(profile.processor, profile.accelerator, exchange)

    return exchange
