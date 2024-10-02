from exchange import Message, Exchange
from exchange.providers import OpenAiProvider

from goose.toolkit import Toolkit
from goose.toolkit.base import tool
from goose.toolkit.summarization.utils import summarize_file
from goose.utils.ask import ask_an_ai, clear_exchange
import PyPDF2


class O1Plan(Toolkit):
    """Make a plan for a project using gpt-o1"""

    # ideas to try:
    # - let goose inject the relevant context into the prompt
    # - make the developer toolkit a requirement for this toolkit to share state between the two

    def system(self):
        return Message.load("prompts/o1_plan.jinja").text

    def create_plan_with_o1(self, user_input: str):
        """This tool generates a plan for a project using the GPT-O1 model.
        """

        provider = OpenAiProvider.from_env()
        accelerator = Exchange(provider=provider, model="gpt-4o-mini", system="")

        # give o1 some context of the project -- right now I hardcoded it in to include a summary of goose and the memgpt paper (because I am working on trying to give goose self-defined context management)
        with open("../../../.goose/summaries/goose-public-summary.json") as f:
            project_summary = f.read()

        # read the pdf
        with open("../../../memgpt.pdf", "rb") as file:
            reader = PyPDF2.PdfReader(file)

            # Iterate over each page
            memgpt = ""
            for page in reader.pages:
                memgpt += page.extract_text()

        with open("memgpt_to_text.txt", "w") as f:
            f.write(memgpt)

        # summarize this paper
        memgpt_summary = summarize_file("memgpt_to_text.txt", accelerator, prompt="Please summarize this paper.")

        related_context = f"{project_summary}\n\n memgpt paper: {memgpt_summary}"

        # tell o1 what you want to code up / do -- this is the prompt
        # open the jinja file and insert the user_input into the template
        system = "I am a developer that can create discrete steps to solve a problem. I propose code changes and additions."

        problem = Message.load("prompts/o1_plan.jinja", **{"user_input": user_input, "related_context": related_context}).text

        print(problem)

        provider = OpenAiProvider.from_env()
        exchange = Exchange(provider=provider, model="o1-preview", system=system)

        # o1 will respond -- the response is the plan
        response = ask_an_ai(input="please help reason about this: " + problem, exchange=exchange, no_history=False)
        print(response.content[0])

        # take the response from o1 and convert it into something that can be used as a plan
        return response

    @tool
    def convert_plan(self, plan_path: str):
        """Convert a plan from GPT-O1 into a more structured format that you can follow.

        Args:
            plan_path (str): The path to the plan file created by o1
        """

        with open(plan_path, "r") as f:
            plan = f.read()

        return plan


