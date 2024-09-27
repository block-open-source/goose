# Available Toolkits in Goose

Goose provides a variety of toolkits designed to help developers with different tasks. Here's an overview of each available toolkit and its functionalities:

## 1. Developer Toolkit

The **Developer** toolkit offers general-purpose development capabilities, including:

- **System Configuration Details:** Retrieves system configuration details.
- **Task Management:** Update the plan by overwriting all current tasks.
- **File Operations:**
  - `patch_file`: Patch a file by replacing specific content.
  - `read_file`: Read the content of a specified file.
  - `write_file`: Write content to a specified file.
- **Shell Command Execution:** Execute shell commands with safety checks.

## 2. GitHub Toolkit

The **GitHub** toolkit provides detailed configuration and procedural guidelines for GitHub operations.

## 3. Lint Toolkit

The **Lint** toolkit ensures that all toolkits have proper documentation. It performs the following checks:

- Toolkit must have a docstring.
- The first line of the docstring should contain more than 5 words and fewer than 12 words.
- The first letter of the docstring should be capitalized.

## 4. RepoContext Toolkit

The **RepoContext** toolkit provides context about the current repository. It includes:

- **Repository Size:** Get the size of the repository.
- **Monorepo Check:** Determine if the repository is a monorepo.
- **Project Summarization:** Summarize the current project based on the repository or the current project directory.

## 5. Screen Toolkit

The **Screen** toolkit assists users in taking screenshots for debugging or designing purposes. It provides:

- **Take Screenshot:** Capture a screenshot and provide the path to the screenshot file.
- **System Instructions:** Instructions on how to work with screenshots.

## 6. SummarizeRepo Toolkit

The **SummarizeRepo** toolkit helps in summarizing a repository. It includes:

- **Summarize Repository:** Clone the repository (if not already cloned) and summarize the files based on specified extensions.

## 7. SummarizeProject Toolkit

The **SummarizeProject** toolkit generates or retrieves a summary of a project directory based on specified file extensions. It includes:

- **Get Project Summary:** Generate or retrieve a summary of the project in the specified directory.

## 8. SummarizeFile Toolkit

The **SummarizeFile** toolkit helps in summarizing a specific file. It includes:

- **Summarize File:** Summarize the contents of a specified file with optional instructions.
