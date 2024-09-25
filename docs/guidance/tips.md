##  Tips for working with Goose:

Here are some collected tips we have for working efficiently with Goose

- **Goose can and will edit files**. Use a git strategy to avoid losing anything - such as staging your
personal edits and leaving Goose edits unstaged until reviewed. Or consider using individual commits which can be reverted.
- **Goose can and will run commands**. You can ask it to check with you first if you are concerned. It will check commands for safety as well.
- You can interrupt Goose with `CTRL+C` to correct it or give it more info.
- Goose works best when solving concrete problems - experiment with how far you need to break that problem
down to get Goose to solve it. Be specific! E.g. it will likely fail to `"create a banking app"`,
but probably does a good job if prompted with `"create a Fastapi app with an endpoint for deposit and withdrawal and with account balances stored in mysql keyed by id"`
- If Goose doesn't have enough context to start with, it might go down the wrong direction. Tell it
to read files that you are referring to or search for objects in code. Even better, ask it to summarize
them for you, which will help it set up its own next steps.
- Refer to any objects in files with something that is easy to search for, such as `"the MyExample class"
- Goose *loves* to know how to run tests to get a feedback loop going, just like you do. If you tell it how you test things locally and quickly, it can make use of that when working on your project
- You can use Goose for tasks that would require scripting at times, even looking at your screen and correcting designs/helping you fix bugs, try asking it to help you in a way you would ask a person.
- Goose will make mistakes, and go in the wrong direction from times, feel free to correct it, or start again.
- You can tell Goose to run things for you continuously (and it will iterate, try, retry) but you can also tell it to check with you before doing things (and then later on tell it to go off on its own and do its best to solve).
- Goose can run anywhere, doesn't have to be in a repo, just ask it!