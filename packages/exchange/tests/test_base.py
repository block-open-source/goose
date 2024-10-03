from exchange.providers.base import MissingProviderEnvVariableError


def test_missing_provider_env_variable_error_without_instructions_url():
    env_variable = "API_KEY"
    provider = "TestProvider"
    error = MissingProviderEnvVariableError(env_variable, provider)

    assert error.env_variable == env_variable
    assert error.provider == provider
    assert error.instructions_url is None
    assert error.message == "Missing environment variable: API_KEY for provider TestProvider."


def test_missing_provider_env_variable_error_with_instructions_url():
    env_variable = "API_KEY"
    provider = "TestProvider"
    instructions_url = "http://example.com/instructions"
    error = MissingProviderEnvVariableError(env_variable, provider, instructions_url)

    assert error.env_variable == env_variable
    assert error.provider == provider
    assert error.instructions_url == instructions_url
    assert error.message == (
        "Missing environment variable: API_KEY for provider TestProvider.\n"
        " Please see http://example.com/instructions for instructions"
    )
