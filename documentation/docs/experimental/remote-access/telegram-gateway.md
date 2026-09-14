---
title: Telegram Gateway
sidebar_position: 2
sidebar_label: Telegram Gateway
description: Chat with goose through Telegram from any device.
---

The Telegram Gateway lets you interact with goose through Telegram, enabling remote access from any device where Telegram is available.

:::warning Experimental Feature
The Gateway feature is experimental and in active development. Behavior and configuration may change in future releases.
:::

## How It Works

The Gateway connects your Telegram account to goose through a secure pairing process. Once paired, you can send messages to a Telegram bot that forwards them to goose, and receive formatted responses back in Telegram.

**Key details:**
- Uses a Telegram bot that you create and configure
- Secure pairing with a one-time code
- Supports formatted responses with code blocks and markdown
- Maintains a persistent session that auto-compacts for long conversations
- Works from any device with Telegram installed

## Prerequisites

Before setting up the Telegram Gateway:

1. [Configure goose](/docs/getting-started/providers) with a provider and model.
2. Open Telegram and search for [@BotFather](https://t.me/BotFather).
3. Send `/newbot` and follow the prompts to create your bot.
4. Copy the **bot token** that BotFather provides (it looks like `123456789:ABCdefGHIjklMNOpqrsTUVwxyz`).

:::tip
Keep your bot token secure. Anyone with the token can control your bot.
:::

## Setup

Use the goose CLI to start the gateway and pair your Telegram account.

### Start the Gateway

In one terminal, export your bot token and start the gateway:

```bash
export TELEGRAM_BOT_TOKEN="YOUR_BOT_TOKEN"
goose gateway start telegram --bot-token "$TELEGRAM_BOT_TOKEN"
```

Leave this command running. Your computer must remain awake and online for the bot to respond.

### Pair Your Telegram Account

In a second terminal, generate a pairing code:

```bash
goose gateway pair telegram
```

Open your bot in Telegram and send it the six-character pairing code within five minutes. The bot confirms when pairing is complete, and you can then chat with goose through Telegram.

To stop the gateway, press <kbd>Ctrl</kbd>+<kbd>C</kbd> in the terminal where it is running.

## What You Can Do

Once paired, you can:
- Send messages to goose and receive responses
- Get formatted code blocks with syntax highlighting
- Continue conversations across multiple sessions
- Access your configured goose extensions

## Troubleshooting

### Bot not responding
- Verify the bot token is correct.
- Check that the `goose gateway start` command is still running.
- Ensure your computer is awake and online.

### Pairing code not working
- Pairing codes expire after five minutes. Generate a new one and try again.
- Make sure you're sending the code to the correct bot.

### Messages not formatting correctly
- The gateway converts goose's markdown to Telegram-compatible formatting.
- Some complex formatting may be simplified for Telegram compatibility.

## Additional Resources

- [Telegram Bot API Documentation](https://core.telegram.org/bots)
- [Gateway PR #7199](https://github.com/aaif-goose/goose/pull/7199)
