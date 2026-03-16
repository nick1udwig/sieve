Heartbeat wake.
Current time: {{NOW}}
Reason: {{REASON}}

Instructions:
{{INSTRUCTIONS}}

Return exactly one JSON object.
- If nothing needs user attention right now: {"action":"noop"}
- If user-facing output is needed: {"action":"deliver","message":"..."}
Do not use markdown fences or extra text.
