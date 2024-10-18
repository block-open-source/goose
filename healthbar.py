import time
import threading

class HealthBar:
    def __init__(self, sessionName, showHealthBar=True):
        self.sessionName = sessionName
        self.exchangeTokenCount = 0
        self.totalCost = 0.0
        self.sessionStartTime = time.time()
        self.isRunning = False
        self.isGenerating = False
        self.showHealthBar = showHealthBar
        self._stopFlag = threading.Event()

    def startSession(self):
        self.isRunning = True
        self.sessionStartTime = time.time()

    def stopSession(self):
        self.isRunning = False

    def startGenerating(self):
        self.isGenerating = True

    def stopGenerating(self):
        self.isGenerating = False

    def addTokens(self, count):
        self.exchangeTokenCount += count

    def addCost(self, cost):
        self.totalCost += cost

    def getSessionUptime(self):
        if not self.isRunning:
            return 0
        return time.time() - self.sessionStartTime

    def displayHealthBar(self):
        if not self.showHealthBar:
            return

        while not self._stopFlag.is_set():
            uptime = self.getSessionUptime()
            status = f"Session: {self.sessionName} | Tokens: {self.exchangeTokenCount} | Cost: ${self.totalCost:.2f} | Uptime: {uptime:.2f}s | Running: {self.isRunning} | Generating: {self.isGenerating}"
            print(status, end="\r")
            time.sleep(1)  # Update every second

    def stopDisplay(self):
        self._stopFlag.set()
