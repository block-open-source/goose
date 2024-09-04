from .developer.developer import Developer  # noqa: F401
from .repo_context.repo_context import RepoContext  # noqa: F401
from .summarization.summarize_repo import SummarizeRepo  # noqa
from .summarization.summarize_project import SummarizeProject  # noqa
from .summarization.summarize_file import SummarizeFile  # noqa: F401
from .github.github import Github  # noqa: F401
from .screen import Screen  # noqa: F401


from functools import cache

from goose.pluginbase.toolkit import Toolkit
from goose.pluginbase.utils import load_plugins


@cache
def get_toolkit(name: str) -> type[Toolkit]:
    return load_plugins(group="goose.toolkit")[name]
