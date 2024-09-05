import random
import string

def random_session_name() -> str:
    return "".join(
        [
            random.choice(string.ascii_lowercase),
            random.choice(string.digits),
            random.choice(string.ascii_lowercase),
            random.choice(string.digits),
        ]
    )