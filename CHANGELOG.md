# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.9.5] - 2024-10-15
- chore: updates ollama default model from mistral-nemo to qwen2.5 (#150)
- feat: add vision support for Google (#141)
- fix: session resume with arg handled incorrectly (#145)
- docs: add release instructions to CONTRIBUTING.md (#143)
- docs: add link to action, IDE words (#140)
- docs: goosehints doc fix only (#142)

## [0.9.4] - 2024-10-10

- revert: "feat: add local langfuse tracing option (#106)"
- feat: add local langfuse tracing option (#106)
- feat: add groq provider (#134)
- feat: add a deep thinking reasoner model (o1-preview/mini) (#68)
- fix: use concrete SessionNotifier (#135)
- feat: add guards to session management (#101)
- fix: Set default model configuration for the Google provider. (#131)
- test: convert Google Gemini tests to VCR (#118)
- chore: Add goose providers list command (#116)
- docs: working ollama for desktop (#125)
- docs: format and clean up warnings/errors (#120)
- docs: update deploy workflow (#124)
- feat: Implement a goose run command (#121)
- feat: saved api_key to keychain for user (#104)
- docs: add callout plugin (#119)
- chore: add a page to docs for Goose application examples (#117)
- fix: exit the goose and show the error message when provider environment variable is not set (#103)
- fix: Update OpenAI pricing per https://openai.com/api/pricing/ (#110)
- fix: update developer tool prompts to use plan task status to match allowable statuses update_plan tool call (#107)
- fix: removed the panel in the output so that the user won't have unnecessary pane borders in the copied content (#109)
- docs: update links to exchange to the new location (#108)
- chore: setup workspace for exchange (#105)
- fix: resolve uvx when using a git client or IDE (#98)
- ci: add include-markdown for mkdocs (#100)
- chore: fix broken badge on readme (#102)
- feat: add global optional user goosehints file (#73)
- docs: update docs (#99)

## [0.9.3] - 2024-09-25

- feat: auto save sessions before next user input (#94)
- fix: removed the diff when default profile changes (#92)
- feat: add shell-completions subcommand (#76)
- chore: update readme! (#96)
- chore: update docs again (#77)
- fix: remove overly general match for long running commands (#87)
- fix: default ollama to tested model (#88)
- fix: Resize file in screen toolkit (#81)
- fix: enhance shell() to know when it is interactive (#66)
- docs: document how to run goose fully from source from any dir (#83)
- feat: track cost and token usage in log file (#80)
- chore: add link to docs in read me (#85)
- docs: add in ollama (#82)
- chore: add just command for releasing goose (#55)
- feat: support markdown plans  (#79)
- feat: add version options (#74)
- docs: fixing exchange url to public version (#67)
- docs: Update CONTRIBUTING.md (#69)
- chore: create mkdocs for goose (#70)
- docs: fix broken link (#71)
- feat: give commands the ability to execute logic (#63)
- feat: jira toolkit (#59)
- feat: run goose in a docker-style sandbox (#44)

## [0.9.0] - 2024-09-10

This also updates the minimum version of exchange to 0.9.0.

- fix: goose should track files it reads and not overwrite changes (#46)
- docs: Small dev notes for using exchange from source (#50)
- fix: typo in exchange method rewind (#54)
- fix: remove unsafe pop of messages (#47)
- chore: Update LICENSE (#53)
- chore(docs): update is_dangerous_command method description (#48)
- refactor: improve safety rails speed and prompt (#45)
- feat: make goosehints jinja templated (#43)
- ci: enforce PR title follows conventional commit (#14)
- feat: show available toolkits (#37)
- feat: adding in ability to provide per repo hints (#32)
- chore: apply ruff and add to CI (#40)
- feat: added some regex based checks for dangerous commands (#38)
- chore: Update publish github workflow to check package versions before publishing (#19)
