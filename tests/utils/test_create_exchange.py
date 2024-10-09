import os
from unittest.mock import MagicMock, patch

from exchange.exchange import Exchange
from exchange.invalid_choice_error import InvalidChoiceError
from exchange.providers.base import MissingProviderEnvVariableError
import pytest

from goose.notifier import Notifier
from goose.profile import Profile
from goose.utils._create_exchange import create_exchange

TEST_PROFILE = MagicMock(spec=Profile)
TEST_EXCHANGE = MagicMock(spec=Exchange)
TEST_NOTIFIER = MagicMock(spec=Notifier)


@pytest.fixture
def mock_print():
    with patch("goose.utils._create_exchange.print") as mock_print:
        yield mock_print


@pytest.fixture
def mock_prompt():
    with patch("goose.utils._create_exchange.prompt") as mock_prompt:
        yield mock_prompt


@pytest.fixture
def mock_confirm():
    with patch("goose.utils._create_exchange.confirm") as mock_confirm:
        yield mock_confirm


@pytest.fixture
def mock_sys_exit():
    with patch("sys.exit") as mock_exit:
        yield mock_exit


@pytest.fixture
def mock_keyring_get_password():
    with patch("keyring.get_password") as mock_get_password:
        yield mock_get_password


@pytest.fixture
def mock_keyring_set_password():
    with patch("keyring.set_password") as mock_set_password:
        yield mock_set_password


def test_create_exchange_success(mock_print):
    with patch("goose.utils._create_exchange.build_exchange", return_value=TEST_EXCHANGE):
        assert create_exchange(profile=TEST_PROFILE, notifier=TEST_NOTIFIER) == TEST_EXCHANGE


def test_create_exchange_fail_with_invalid_choice_error(mock_print, mock_sys_exit):
    expected_error = InvalidChoiceError(
        attribute_name="provider", attribute_value="wrong_provider", available_values=["openai"]
    )
    with patch("goose.utils._create_exchange.build_exchange", side_effect=expected_error):
        create_exchange(profile=TEST_PROFILE, notifier=TEST_NOTIFIER)

        assert "Unknown provider: wrong_provider. Available providers: openai" in mock_print.call_args_list[0][0][0]
        mock_sys_exit.assert_called_once_with(1)


class TestWhenProviderEnvVarNotFound:
    API_KEY_ENV_VAR = "OPENAI_API_KEY"
    API_KEY_ENV_VALUE = "api_key_value"
    PROVIDER_NAME = "openai"
    SERVICE_NAME = "goose"
    EXPECTED_ERROR = MissingProviderEnvVariableError(env_variable=API_KEY_ENV_VAR, provider=PROVIDER_NAME)

    def test_create_exchange_get_api_key_from_keychain(
        self, mock_print, mock_sys_exit, mock_keyring_get_password, mock_keyring_set_password
    ):
        self._clean_env()
        with patch("goose.utils._create_exchange.build_exchange", side_effect=[self.EXPECTED_ERROR, TEST_EXCHANGE]):
            mock_keyring_get_password.return_value = self.API_KEY_ENV_VALUE

            assert create_exchange(profile=TEST_PROFILE, notifier=TEST_NOTIFIER) == TEST_EXCHANGE

            assert os.environ[self.API_KEY_ENV_VAR] == self.API_KEY_ENV_VALUE
            mock_keyring_get_password.assert_called_once_with(self.SERVICE_NAME, self.API_KEY_ENV_VAR)
            mock_print.assert_called_once_with(
                f"Using {self.API_KEY_ENV_VAR} value for {self.PROVIDER_NAME} from your keychain"
            )
            mock_sys_exit.assert_not_called()
            mock_keyring_set_password.assert_not_called()

    def test_create_exchange_ask_api_key_and_user_set_in_keychain(
        self, mock_prompt, mock_confirm, mock_sys_exit, mock_keyring_get_password, mock_keyring_set_password, mock_print
    ):
        self._clean_env()
        with patch("goose.utils._create_exchange.build_exchange", side_effect=[self.EXPECTED_ERROR, TEST_EXCHANGE]):
            mock_keyring_get_password.return_value = None
            mock_prompt.return_value = self.API_KEY_ENV_VALUE
            mock_confirm.return_value = True

            assert create_exchange(profile=TEST_NOTIFIER, notifier=TEST_NOTIFIER) == TEST_EXCHANGE

            assert os.environ[self.API_KEY_ENV_VAR] == self.API_KEY_ENV_VALUE
            mock_keyring_set_password.assert_called_once_with(
                self.SERVICE_NAME, self.API_KEY_ENV_VAR, self.API_KEY_ENV_VALUE
            )
            mock_confirm.assert_called_once_with(
                f"Would you like to save the {self.API_KEY_ENV_VAR} value to your keychain?"
            )
            mock_print.assert_called_once_with(
                f"Saved {self.API_KEY_ENV_VAR} to your key_chain. "
                + f"service_name: goose, user_name: {self.API_KEY_ENV_VAR}"
            )
            mock_sys_exit.assert_not_called()

    def test_create_exchange_ask_api_key_and_user_not_set_in_keychain(
        self, mock_prompt, mock_confirm, mock_sys_exit, mock_keyring_get_password, mock_keyring_set_password
    ):
        self._clean_env()
        with patch("goose.utils._create_exchange.build_exchange", side_effect=[self.EXPECTED_ERROR, TEST_EXCHANGE]):
            mock_keyring_get_password.return_value = None
            mock_prompt.return_value = self.API_KEY_ENV_VALUE
            mock_confirm.return_value = False

            assert create_exchange(profile=TEST_NOTIFIER, notifier=TEST_NOTIFIER) == TEST_EXCHANGE

            assert os.environ[self.API_KEY_ENV_VAR] == self.API_KEY_ENV_VALUE
            mock_keyring_set_password.assert_not_called()
            mock_sys_exit.assert_not_called()

    def test_create_exchange_fails_when_user_not_provide_api_key(
        self, mock_prompt, mock_confirm, mock_sys_exit, mock_keyring_get_password, mock_print
    ):
        self._clean_env()
        with patch("goose.utils._create_exchange.build_exchange", side_effect=self.EXPECTED_ERROR):
            mock_keyring_get_password.return_value = None
            mock_prompt.return_value = None
            mock_confirm.return_value = False

            create_exchange(profile=TEST_NOTIFIER, notifier=TEST_NOTIFIER)

            assert (
                "Please set the required environment variable to continue."
                in mock_print.call_args_list[0][0][0].renderable
            )
            mock_sys_exit.assert_called_once_with(1)

    def _clean_env(self):
        os.environ.pop(self.API_KEY_ENV_VAR, None)
