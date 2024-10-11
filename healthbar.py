# health_bar.py
import time
import threading

class HealthBar:
    def __init__(self, session_name, show_health_bar=True):
        self.session_name = session_name
        self.exchange_token_count = 0
        self.total_cost = 0.0
        self.session_start_time = time.time()
        self.is_running = False
        self.is_generating = False
        self.show_health_bar = show_health_bar
        self._stop_flag = threading.Event()

    def start_session(self):
        self.is_running = True
        self.session_start_time = time.time()

    def stop_session(self):
        self.is_running = False

    def start_generating(self):
        self.is_generating = True

    def stop_generating(self):
        self.is_generating = False

    def add_tokens(self, count):
        self.exchange_token_count += count

    def add_cost(self, cost):
        self.total_cost += cost

    def get_session_uptime(self):
        if not self.is_running:
            return 0
        return time.time() - self.session_start_time

    def display_health_bar(self):
        if not self.show_health_bar:
            return

        while not self._stop_flag.is_set():
            uptime = self.get_session_uptime()
            status = f"Session: {self.session_name} | Tokens: {self.exchange_token_count} | Cost: ${self.total_cost:.2f} | Uptime: {uptime:.2f}s | Running: {self.is_running} | Generating: {self.is_generating}"
            print(status, end="\r")
            time.sleep(1)  # Update every second

    def stop_display(self):
        self._stop_flag.set()
