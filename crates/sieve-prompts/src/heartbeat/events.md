Main-session system events are pending.
Current time: {{NOW}}
Reason: {{REASON}}
{{INSTRUCTIONS_BLOCK}}

Queued events:
{{QUEUED_EVENTS}}

Handle the queued events now.
Return exactly one JSON object.
- If nothing needs user attention right now: {"action":"noop"}
- If user-facing output is needed: {"action":"deliver","message":"..."}
Do not use markdown fences or extra text.
