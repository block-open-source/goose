from goose._internal.utils import load_plugins


def test_load_plugins():
    plugins = load_plugins("exchange.provider")
    assert isinstance(plugins, dict)
    assert len(plugins) > 0
