# Hone Onboarding Hub

Welcome to Hone. Whether you're new to AI coding agents or migrating from another tool, this hub will guide you through setup and best practices.

## Quick Navigation

### I'm Coming From Another Tool

Choose your current tool to see what maps to what in Hone:

| Your Tool | Read This |
|-----------|-----------|
| Claude Code (Anthropic) | [from-claude-code.md](from-claude-code.md) |
| Goose (Block/Square) | [from-goose.md](from-goose.md) |
| Aider | [from-aider.md](from-aider.md) |
| Codex CLI | [from-codex-cli.md](from-codex-cli.md) |
| OpenCode | [from-opencode.md](from-opencode.md) |

### I'm New to AI Coding Agents

Start here: [new-user-walkthrough.md](new-user-walkthrough.md)

This guide assumes zero prior experience and covers:
- What an AI coding agent actually is
- Installing Hone
- Your first real-world session
- Key concepts explained simply
- Common workflows and patterns
- Effective prompting tips

## Decision Tree: Which Guide Is For Me?

```
Are you completely new to AI coding agents?
├─ YES → Read: new-user-walkthrough.md
└─ NO
   Have you used an AI coding tool before?
   ├─ NO → Read: new-user-walkthrough.md (covers fundamentals)
   └─ YES
      Which tool?
      ├─ Claude Code → from-claude-code.md
      ├─ Goose → from-goose.md
      ├─ Aider → from-aider.md
      ├─ Codex CLI → from-codex-cli.md
      ├─ OpenCode → from-opencode.md
      └─ Something else → new-user-walkthrough.md (general intro)
```

## What Makes Hone Different?

At a high level, Hone brings together ideas from all competing tools:

**From Claude Code:** Extended thinking, agent loops, memory, user-defined conventions (HONE.md)

**From Goose:** MCP-native architecture, YAML recipes, provider abstraction

**From Aider:** Git-centric workflows, repo context, semantic indexing

**From Codex CLI:** Kernel sandboxing, approval workflows, cost tracking

**From OpenCode:** 75+ provider support, LSP integration, client-server architecture

But Hone goes further:
- **Kernel-enforced security** — Seatbelt on macOS, Landlock on Linux
- **Adaptive memory** — Learns what works across sessions
- **Universal provider support** — No vendor lock-in
- **Layered tools** — Native Rust for speed, MCP for extensibility
- **Collaborative workflows** — Remote daemon with team governance

## Common Questions

### Do I need an API key?

No, but it helps. Hone supports:
- **Free local models** via Ollama (run on your machine)
- **Your existing CLI** via Claude/Gemini pass-through
- **Any API provider** (Anthropic, OpenAI, etc.)

See [Getting Started](../tutorials/getting-started.md) for setup.

### Is my code private?

Yes. Hone:
- Runs locally (no telemetry)
- Stores memory in encrypted SQLite on your machine
- Sandboxes code execution at the kernel level
- Never phones home

See [Security Guide](../security.md) for details.

### Can I use Hone in a team?

Yes. Run the `honed` daemon and connect multiple users with role-based access control (RBAC). See [Server and Remote](../tutorials/server-and-remote.md).

### How much does Hone cost?

Hone is free. You pay for model APIs (if you use them). Costs are transparent and tracked in real USD. Local models cost nothing.

### What if I switch providers?

Easy. Use `/model <provider:model>` to hot-swap mid-session without losing history. See [Providers Guide](../tutorials/providers.md).

## Next Steps After Onboarding

Once you've picked your migration guide and completed setup:

1. **Try the first example** in the migration guide (copy-paste commands)
2. **Run a real-world task** in your own codebase
3. **Explore recipes** in [Recipes Guide](../tutorials/recipes.md) for automation
4. **Review [Tools and MCP](../tutorials/tools-and-mcp.md)** to extend capabilities
5. **Check [Architecture](../architecture.md)** for deep dives into how Hone works

## Getting Help

- **Setup issues?** See the "Troubleshooting" section in [Getting Started](../tutorials/getting-started.md)
- **Questions about concepts?** See [Architecture](../architecture.md)
- **Want to extend Hone?** See [Contributing](../contributing.md)
- **Found a bug?** Report it on GitHub

---

Have a migration guide question we missed? Open an issue on GitHub with the "docs" label.
