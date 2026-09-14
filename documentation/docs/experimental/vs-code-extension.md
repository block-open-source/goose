---
title: goose for VS Code Extension
sidebar_label: VS Code Extension
sidebar_position: 4
---

import IconGoose from "@site/src/components/icons/goose";

The [goose for VS Code extension](https://marketplace.visualstudio.com/items?itemName=block.vscode-goose) lets you interact with goose directly within VS Code.

:::warning Experimental Feature
goose for VS Code is in active development. Behavior and configuration may change in future releases.
:::

The extension uses the [Agent Client Protocol (ACP)](/docs/gdk/acp) to communicate with goose.

## Install the Extension

:::info Prerequisites
- [VS Code](https://code.visualstudio.com/) version 1.95.0 or higher
- [goose CLI](/docs/getting-started/installation)
:::

1. Go to [Visual Studio Marketplace](https://marketplace.visualstudio.com/items?itemName=block.vscode-goose)
2. Click `Install`
3. On the popup window, click `Open Visual Studio Code`
4. In Visual Studio Code, click `Install`
5. Click the <IconGoose className="inline" size={16} /> icon in the Activity Bar to open the side panel and start chatting with goose

## Share Context with goose

There are two ways to provide context to goose.

### Send Selected Code

1. Select the code you want to discuss in your editor
2. Send the selection to goose:
   - macOS: `Cmd+Shift+G`
   - Windows/Linux: `Ctrl+Shift+G`
   - Right-click and choose `Send to goose`

The selected code will appear as a context chip above the chat input.

### Attach Files with @ Mentions

1. Type `@` in the chat input and start typing the file name
2. Select the file:
   - Use the arrow keys (↑/↓) to select the file and press `Enter`
   - Use your mouse to select the file

The attached file will appear as a context chip above the chat input.

## Example Usage

The goose for VS Code extension connects directly to your editor, letting you manage files, projects, and workflows with AI. It includes the following features:

- Interactive chat UI with streaming responses
- Access to goose's AI capabilities and enabled extensions
- Session management with history replay
- Context sharing and visual context chips in the chat UI
- Clipboard tools to copy code snippets/responses for easy sharing

### goose Prompt

```
Update the contributing guide at /goose/CONTRIBUTING.md with instructions on how to start docusaurus dev server
```

### goose Output

```

I'll help you update the CONTRIBUTING.md file with instructions for starting the Docusaurus dev server. First, let me check the current content of the file.

─── developer__text_editor | view ──────────────────────────

I'll now update the CONTRIBUTING.md file to include instructions for starting the Docusaurus dev server. I'll add this information in a new section under the Node section.

─── developer__text_editor | str_replace ──────────────────────────

I've successfully updated the CONTRIBUTING.md file to include instructions for starting the Docusaurus dev server. I added a new "Documentation" section right after the Node section, which provides clear step-by-step instructions for:

Navigating to the docs directory
Installing dependencies
Starting the development server
I also included helpful information about the hot-reloading feature and the local server URL. The formatting is consistent with the rest of the document, using proper markdown syntax for code blocks and headers.

Is there anything else you'd like me to add or modify in the contributing guide?

```
