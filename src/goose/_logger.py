import logging
from pathlib import Path
import json
from typing import Union
from exchange import Message, ToolResult

_LOGGER_NAME = "goose"
_LOGGER_FILE_NAME = "goose.log"

_TRACE_LOGGER_NAME = "trace_logger"
_TRACE_LOGGER_FILE_NAME = "trace.log"

class TraceFilter(logging.Filter):
    def __init__(self, toolResultOutputMaxTokens=1000):
        super().__init__()
        self.toolResultOutputMaxTokens = toolResultOutputMaxTokens

    def filter(self, record):
        if hasattr(record, 'trace_contents'):
            record.msg = self.parse_trace_message(record.trace_contents)
        return True  

    def parse_trace_message(self, message: Union[str, dict, Message, ToolResult]):
        # Custom parsing logic for trace messages
        logMsg = ""
        try:
            if isinstance(message, Message):
                logMsg += f"{message.role}: {message.__class__.__name__}\n"
                for content in message.content:
                    logMsg += json.dumps(content.to_dict(), indent=4) + "\n\n"
            elif isinstance(message, ToolResult):
                logMsg += f"{message.__class__.__name__}\n"
                if message.is_error:
                    logMsg += " ********** ERROR **********\n"
                
                message.output = message.output.replace("\\n", "\n")
                if len(message.output) > self.toolResultOutputMaxTokens:
                    message.output = message.output[:self.toolResultOutputMaxTokens] + "...[TRUNCATED]..."
                logMsg += "\n" + json.dumps(message.to_dict(), indent=4, ensure_ascii=False) + "\n\n"
                
            elif isinstance(message, dict):
                logMsg += json.dumps(message, indent=4) + "\n\n"

            elif isinstance(message, str):
                logMsg += message + "\n\n"

            else:
                logMsg += f"Unhandled trace message type: {type(message)}\n\n"
        except Exception as e:
            logMsg += f"Exception raised in trace logging: {e}\n"

        return logMsg


def setup_logging(log_file_directory: Path, log_level: str = "INFO") -> None:
    logger = logging.getLogger(_LOGGER_NAME)
    logger.setLevel(getattr(logging, log_level))
    log_file_directory.mkdir(parents=True, exist_ok=True)
    file_handler = logging.FileHandler(log_file_directory / _LOGGER_FILE_NAME)
    logger.addHandler(file_handler)
    formatter = logging.Formatter("%(asctime)s - %(name)s - %(levelname)s - %(message)s")
    file_handler.setFormatter(formatter)

    trace_logger = logging.getLogger(_TRACE_LOGGER_NAME)
    trace_logger.setLevel(logging.DEBUG)
    trace_handler = logging.FileHandler(log_file_directory / _TRACE_LOGGER_FILE_NAME)
    trace_handler.addFilter(TraceFilter(toolResultOutputMaxTokens=500))
    trace_logger.addHandler(trace_handler)
    trace_handler.setFormatter(formatter)


def get_logger() -> logging.Logger:
    return logging.getLogger(_LOGGER_NAME)


def get_trace_logger() -> logging.Logger:
    return logging.getLogger(_TRACE_LOGGER_NAME)
