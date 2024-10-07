from exchange import Exchange, Message
from exchange.providers.openai import OpenAiProvider

from goose.notifier import Notifier
from goose.toolkit.o1_plan import O1Plan
from goose.utils.ask import ask_an_ai

class LoggingNotifier(Notifier):
    def log(self, message):
        print(message)

    def status(self, message):
        print(message)

    def start(self):
        pass

    def stop(self):
        pass


def test_ask_o1():
    print("thinking...")

    o1_plan = O1Plan(notifier=LoggingNotifier())

    #  -- give it tools to shuffle things in and out of its context and store and lookup things from external memory. Write a plan to implement this
    resp = o1_plan.create_plan_with_o1("I want to make goose into a memgpt model -- give it tools to shuffle things in and out of its context and store and lookup things from external memory.", relevant_files=["memgpt_to_text.txt"])

    print("done thinking")

    print(resp.content[0].text)

    with open("o1_response_2.txt", "w") as f:
        f.write(resp.content[0].text)

