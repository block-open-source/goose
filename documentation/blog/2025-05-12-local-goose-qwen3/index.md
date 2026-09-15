---
title: "Goose and Qwen3 for Local Execution"
description: "Run AI commands locally with Goose and Qwen3 for fast, offline tool execution"
authors: 
    - mic
---

:::warning Outdated
The goose `/plan` mode mentioned under Advanced tips has since been removed, along with the separate planner model settings. Everything else in this post still applies.
:::

![local AI agent](goose-qwen-local.png)


A couple of weeks back, [Qwen 3](https://qwenlm.github.io/blog/qwen3/) launched with a raft of capabilities and sizes. This model showed promise and even in very compact form, such as 8B parameters and 4bit quantization, was able to do tool calling successfully with goose. Even multi turn tool calling. 

I haven't seen this work at such a scaled down model so far, so this is really impressive and bodes well for both this model, but also future open weight models both large and small.  I would expect the Qwen3 larger models work quite well on various tasks but even this small one I found useful.

<!-- truncate -->

## Local workflows and local agents

For some time I have had a little helper function in my `~/.zshrc` file for command line usage: 

```zsh
# zsh helper to use goose if you make a typo or just want to yolo into the shell
command_not_found_handler() {
  local cmd="$*"
  echo "🪿:"
  goose run -t "can you try to run this command please: $cmd"
}
```

This makes use of a zsh feature (zsh now being standard on macos) that will delegate to that function if nothing else on the command line makes sense. 
This lets me either make typos or just type in what I want in the command line such as `$> can you kill whatever is listening on port 8000` and goose will do the work, don't even need to open a goose session.

With Qwen3 + Ollama running all locally with goose, it worked well enough I switched over to a complete local version of that workflow which works when I am offline, on the train etc:

```zsh
command_not_found_handler() {
  local cmd="$*"
  echo "🪿:"
  GOOSE_PROVIDER=ollama GOOSE_MODEL=michaelneale/qwen3 goose run -t "can you try to run this command please: $cmd"
}
```



## Qwen3 reasoning


By default Qwen 3 models will "think" (reason) about the problem, as they are general purpose models, but I found it was quicker (and worked better for my purpose) to make it skip this reasoning stage.

By adding `/no_think` to the system prompt, it will generally skip to the execution (this may make it less successful at larger tasks but this is a small model for just a few turns of tool calls in this case). 

I made a [small tweak to the default Ollama chat template](https://ollama.com/michaelneale/qwen3) which you can use as above that you can use as above, if you like (or the default `qwen3` model hosted by Ollama also works fine out of the box).

## Advanced tips

You can use the goose `/plan` mode with a separate model (perhaps Qwen3 with reasoning, or another model such as deepseek) to help plan actions before shifting to Qwen3 for the execution via tool calls. 

It would be interesting to try the larger models if, you have access to hardware (I have only used the 8B parameter one). My current setup is a 64G M1 pro MacBook (circa 2022 hardware) which has probably less than 48G available to use for GPUs/AI, which puts a limit on what I can run, but qwen3 with "no think" mode works acceptably for my purposes.

<head>
  <meta property="og:title" content="Goose and Qwen3 for Local Execution" />
  <meta property="og:type" content="article" />
  <meta property="og:url" content="https://goose-docs.ai/blog/2025/05/12/local-goose-qwen3" />
  <meta property="og:description" content="Run AI commands locally with Goose and Qwen3 for fast, offline tool execution" />
  <meta property="og:image" content="https://goose-docs.ai/assets/images/goose-qwen-local-62d07cd240ff65cb99a6ef41a2c851a5.png" />
  <meta name="twitter:card" content="summary_large_image" />
  <meta property="twitter:domain" content="goose-docs.ai" />
  <meta name="twitter:title" content="Goose and Qwen3 for Local Execution" />
  <meta name="twitter:description" content="Run AI commands locally with Goose and Qwen3 for fast, offline tool execution" />
  <meta name="twitter:image" content="https://goose-docs.ai/assets/images/goose-qwen-local-62d07cd240ff65cb99a6ef41a2c851a5.png" />
</head>

