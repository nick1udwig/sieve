---
summary: "How Sieve resolves user-facing reply personality across channels and persisted preferences"
read_when:
  - You are modifying reply composition or delivery context.
  - You are debugging why the assistant changed tone, emoji usage, or verbosity.
title: "Personality Layering"
---

# Personality Layering

Sieve now resolves user-facing personality as layered runtime state instead of a single hard-coded prompt string.

The active reply personality is resolved in this order.

1. Base default assistant identity: helpful, optimistic, cheerful, clear, and concise.
2. Channel defaults from delivery context, such as `telegram` chat guidance versus `stdin` plain-text guidance.
3. Persisted user preferences from `SIEVE_HOME/state/personality.json`.
4. Turn-scoped overrides inferred from the current trusted user message.

Current built-in override detection supports common requests such as disabling emojis, requesting lighter or emoji-heavy chat style, asking for more detail, requesting terse or telegraph-like replies, and tone/persona shifts like more formal, more flirty, or “act like a valley girl.”

If a user message is only a personality instruction, Sieve acknowledges the change directly and skips planner execution for that turn.

The resolved personality and delivery context are injected into both the response writer and the compose pass so channel-aware style survives through the final user-facing message.
