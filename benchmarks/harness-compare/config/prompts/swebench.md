You are working in a checked-out copy of the {{repo}} repository at {{workdir}}.

Resolve the following GitHub issue by editing the source code in {{workdir}}.

<issue>
{{problem_statement}}
</issue>

Rules:
- Make the smallest change that fixes the issue. Do not modify existing tests.
- The project's Python environment is already on PATH (`python`, `pytest`). You may
  write small scripts to reproduce the issue and run the relevant tests.
- There is no network access. Do not commit: your working-tree changes are
  collected automatically when you stop.
- When the fix is complete, reply with a one-paragraph summary and stop.
