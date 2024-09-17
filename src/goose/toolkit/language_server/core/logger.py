import logging


class LanguageClientLogger:
    """A console logger for the Language Client"""

    def __init__(self, level=logging.INFO) -> None:
        logging.basicConfig(
            filename="goose-lspclient.log", level=level, format="%(asctime)s - %(name)s - %(levelname)s - %(message)s"
        )
        self.logger = logging.getLogger(self.__class__.__name__)

    def log_request(self, id: int, message: str) -> None:
        message = f"Request ID: {id}. Message: {message}"
        self.logger.info(message)
        print(f"[blue]{message}[/]")

    def log_response(self, id: int, message: str) -> None:
        message = f"Response ID: {id}. Message: {message}"
        self.logger.info(message)
        print(f"[green]{message}[/]")
