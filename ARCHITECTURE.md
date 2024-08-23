# Architecture

## The System

Goose extends the capabilities of high-performing LLMs through a small collection of tools.
This lets you instruct goose, currently via a CLI interface, to automatically solve problems
on your behalf. It attempts to not just tell you how you can do something, but to actually do it for you.

The primary mode of goose (the "developer" toolkit) has access to tools to 

- maintain a plan
- run shell commands
- read, create, and edit files

Together these can solve all kinds of problems, and we emphasize performance on tasks like
fully automating adhoc scripts and tasks, interacting with existing code bases, and teaching how
to use new technology.

---

Here are some of the key design decisions about how we drive performance on these tasks with goose,
that you should be able to observe by using it.

- Encouraging it to write and maintain a plan, to allow it to accomplish longer sequences of automation
- Using tool usage as a generalizable and increasingly tuned approach to adding new capabilities (including plugins)
- Relying on reflection at every possible part of the stack
   - Showing it clear output of each tool use
   - Surfacing all possible errors to the model to give it a chance to correct
   - Surfacing the plan to document what has been accomplished
   
> [!TIP] 
> In addition, there are some implementation choices that we've found very performance driving. The share 
> a theme of working well by default without constraining the model.
> 
> - Encouraging the model to use `ripgrep` via the shell performs very well for navigating filesystems. It mostly 
> just works, but enables the model to get clever with regexes or even additional shell operations as needed. 
> - Using a replace operation for editing files requires fewer tokens to be generated and avoids laziness on large files,
> but we allow fall back to whole file overwrites to let it more coherently handle major refactors.

## Implementation

The core execution logic for generation and tool calling is handled by [exchange][exchange].
It hooks python functions into the model tool use loop, while defining very careful error handling
so any failures in tools are surfaced to the model.

Once we've created an *exchange* object, running the process is effectively just calling 
`exchange.reply()`.

*The key is setting up an exchange with the capabilities we need.*

Goose builds that exchange:
- allows users to configure a profile to customize capabilities
- provides a pluggable system for adding tools and prompts
- sets up the tools to interact with state

We expect that goose will have multiple UXs over time, and be run in different
environments. The UX is expected to be able to load a `Profile` (e.g. in the CLI
we read profiles out of a config) and to provide a `Notifier` (e.g. in the CLI we put
notifications on stdout).

Goose then constructs the exchange for the UX, the UX only interacts with that exchange. 

```
def build_exchange(profile: Profile, notifier: Notifier) -> Exchange:
    ...
```

But to setup a configurable system, Goose uses `Toolkit`s:

```
(Profile, Notifier) -> [Toolkits] -> Exchange 
```

## Profile

A profile specifies some basic configuration in Goose, such as which models it should use, as well
as which toolkits it should include. 

```yaml
processor: openai:gpt-4o
accelerator: openai:gpt-4o-mini
moderator: passive
toolkits:
  - assistant
  - calendar
  - contacts
  - name: scheduling
    requires:
      assistant: assistant
      calendar: calendar
      contacts: contacts
```

## Notifier

The notifier is a concrete implementation of the Notifier base class provided by each UX. It
needs to support two methods

```python
class Notifier:
    def log(self, RichRenderable):
        ...

    def status(self, str):
        ...
```

Log is meant to record something concrete that happened, such as a tool being called, and status is intended
for transient displays of the current status. For example, while a shell command is running, it might use
`.log` to record the command that started, and then update the status to `"shell command running"`. Log is durable
while Status is ephemeral.

## Toolkits

Toolkits are a collection of tools, along with the state and prompting they require. 
Toolkits are what gives Goose its capabilities. 

Tools need a way to report what's happening back to the user, which we treat similarly
to logging. To make that possible, toolkits get a reference to the interface described above.

```python
class ScheduleToolkit(Toolkit):
    def __init__(self, notifier: Notifier, requires: Requirements, **kwargs):
        super().__init__(notifier, requires, **kwargs) # handles the interface, exchangeview
        
        # for a class that has requirements, you can get them like this
        self.calendar = requires.get("calendar")
        self.assistant = requires.get("assistant")
        self.contacts = requires.get("contacts")
        
        self.appointments_state = []

    def prompt(self) -> str:
        return "Try out the example tool."

    @tool
    def example(self):
        self.interface.log(f"An example tool was called, current state is {self.state}")
```

### Advanced

**Dependencies**: Toolkits can depend on each other, to make it easier to get plugins to extend
or modify existing capabilities. In the config above, you can see this used for the scheduling toolkit.
You can refer to those requirements in code through:

```python
@tool
def example_dependency(self):
    appointments = self.dependencies["calendar"].appointments
    ...
```


**ExchangeView**: It can also be useful for tools to have a read-only copy of the history
of the loop so far. So for advanced use cases, toolkits also have access to an 
`ExchangeView` object.

```python
@tool
def example_history(self):
    last_message = self.exchange_view.processor.messages[-1]
    ...
```

[exchange]: https://github.com/squareup/exchange
