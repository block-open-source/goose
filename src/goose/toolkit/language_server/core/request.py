import threading


class LSPRequest:
    def __init__(self, id: int) -> None:
        self.id = id
        self.status = "pending"
        self.event = threading.Event()
        self.response = None
