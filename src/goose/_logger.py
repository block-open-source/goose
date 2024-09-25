import logging
from pathlib import Path
import json
from exchange import Message, ToolResult

_LOGGER_NAME = "goose"
_LOGGER_FILE_NAME = "goose.log"

_TRACE_LOGGER_NAME = "trace_logger"
_TRACE_LOGGER_FILE_NAME = "trace.log"

TRACE_LEVEL = 5
logging.addLevelName(TRACE_LEVEL, "TRACE")


def trace(
    self: logging.Logger, message: dict, toolResultOutputMaxTokens: int = 1000, *args: tuple, **kws: dict
) -> None:
    """
    Log 'message' with severity 'TRACE' to log agent traces.

    Usage: logger.trace("message")
    """
    tempLog = ""
    try:
        if isinstance(message, Message):
            tempLog += f"{message.role}: {message.__class__.__name__}"
            for c in message.content:
                tempLog += "\n" + json.dumps(c.to_dict(), indent=4) + "\n\n"
        elif isinstance(message, ToolResult):
            tempLog += f"{message.__class__.__name__}"
            if message.is_error:
                tempLog += " ********** ERROR **********"
            if len(message.output) > toolResultOutputMaxTokens:
                message.output = message.output[:toolResultOutputMaxTokens] + "...[TRUNCATED]..."
            tempLog += "\n" + json.dumps(message.to_dict(), indent=4).replace("\\n", "\n") + "\n\n"
        else:
            tempLog += "\n" + message + "\n\n"
    except Exception as e:
        tempLog += f"Exception raised in trace logging: {e}"
    if self.isEnabledFor(TRACE_LEVEL):
        self._log(TRACE_LEVEL, tempLog, args, **kws)


logging.Logger.trace = trace


def setup_logging(log_file_directory: Path, log_level: str = "INFO") -> None:
    logger = logging.getLogger(_LOGGER_NAME)
    logger.setLevel(getattr(logging, log_level))
    log_file_directory.mkdir(parents=True, exist_ok=True)
    file_handler = logging.FileHandler(log_file_directory / _LOGGER_FILE_NAME)
    logger.addHandler(file_handler)
    formatter = logging.Formatter("%(asctime)s - %(name)s - %(levelname)s - %(message)s")
    file_handler.setFormatter(formatter)

    trace_logger = logging.getLogger(_TRACE_LOGGER_NAME)
    trace_logger.setLevel(TRACE_LEVEL)
    trace_file_handler = logging.FileHandler(log_file_directory / _TRACE_LOGGER_FILE_NAME)
    trace_logger.addHandler(trace_file_handler)
    trace_file_handler.setFormatter(formatter)


def get_logger() -> logging.Logger:
    return logging.getLogger(_LOGGER_NAME)


def get_trace_logger() -> logging.Logger:
    return logging.getLogger(_TRACE_LOGGER_NAME)
