import pytest
from exchange.providers.utils import get_env_url


def test_url_from_env_valid_http(monkeypatch):
    monkeypatch.setenv("OLLAMA_HOST", "http://localhost:11434/")

    result = get_env_url("OLLAMA_HOST")
    assert str(result) == "http://localhost:11434/"


def test_url_from_env_valid_https(monkeypatch):
    monkeypatch.setenv("OLLAMA_HOST", "https://localhost:11434/v1")

    result = get_env_url("OLLAMA_HOST")
    assert str(result) == "https://localhost:11434/v1"


def test_url_from_env_throw_error_when_empty(monkeypatch):
    monkeypatch.setenv("OLLAMA_HOST", "")

    with pytest.raises(ValueError, match="OLLAMA_HOST was empty"):
        get_env_url("OLLAMA_HOST")


def test_url_from_env_throw_error_when_missing_schemes(monkeypatch):
    monkeypatch.setenv("OLLAMA_HOST", "localhost:11434")

    with pytest.raises(ValueError, match="expected OLLAMA_HOST to be a 'http' or 'https' url: localhost:11434"):
        get_env_url("OLLAMA_HOST")


def test_url_from_env_throw_error_when_invalid_scheme(monkeypatch):
    monkeypatch.setenv("OLLAMA_HOST", "ftp://localhost:11434/v1")

    with pytest.raises(
        ValueError, match="expected OLLAMA_HOST to be a 'http' or 'https' url: ftp://localhost:11434/v1"
    ):
        get_env_url("OLLAMA_HOST")
