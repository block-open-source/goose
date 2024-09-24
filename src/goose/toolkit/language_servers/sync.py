from goose.language_server.base import SyncLanguageServer
from goose.language_server.config import MultilspyConfig
from goose.language_server.implementations.jedi import JediServer
from goose.language_server.logger import MultilspyLogger


sls = SyncLanguageServer(dict(python=JediServer.from_env(config=MultilspyConfig(), logger=MultilspyLogger())))
with sls.start_servers() as _:
    d = sls.request_definition("python", "/Users/lalvoeiro/Development/goose/src/goose/build.py", 41, 28)
    print(d)
