package io.github.aaif_goose.providers.openai

public fun provider(
    apiKey: String,
    baseUrl: String? = null,
): io.github.aaif_goose.Provider = io.github.aaif_goose.openaiProvider(apiKey, baseUrl)

public fun defaultModel(): String = io.github.aaif_goose.openaiDefaultModel()
