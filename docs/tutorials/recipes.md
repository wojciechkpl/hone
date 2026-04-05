# Recipes: Automating Workflows with YAML

Recipes are YAML files that define multi-step agent workflows. Each step runs a prompt through the model, and steps can depend on previous outputs.

## What Are Recipes?

A recipe automates a sequence of agent turns. Instead of typing prompts manually, you define them once in YAML and run them repeatedly.

**When to use recipes:**
- Multi-step workflows (e.g., analyze → improve → test)
- Reproducible processes (e.g., code review, documentation generation)
- Conditional branches (e.g., if test fails, retry)
- TDD: define test before implementation

## Recipe YAML Format

Basic structure:

```yaml
name: "Code Review"
description: "Review a code change and suggest improvements"

steps:
  - name: "Analyze Changes"
    prompt: "Review the changes in this pull request and identify issues"
  
  - name: "Suggest Improvements"
    prompt: "Based on your analysis, list the top 3 improvements we should make"
    when: true  # conditional: only run if condition is true
  
  - name: "Generate Summary"
    prompt: "Write a concise summary of the code review"
    when: true

test_cmd: "cargo test"  # optional: verify the recipe worked
```

### Recipe Fields

| Field | Type | Required | Description |
|---|---|---|---|
| `name` | string | Yes | Recipe identifier |
| `description` | string | No | Human-readable description |
| `steps` | array | Yes | List of steps (see below) |
| `test_cmd` | string | No | Shell command to verify recipe success (TDD) |

### Step Fields

| Field | Type | Required | Description |
|---|---|---|---|
| `name` | string | Yes | Step identifier (used in logs, output) |
| `prompt` | string | Yes | The prompt to send to the agent |
| `when` | bool | No | Conditional execution (default: true) |

## Complete Example

Here's a realistic recipe for implementing a feature with TDD:

```yaml
name: "TDD Feature Implementation"
description: "Implement a feature test-first: write test → implement → verify"

steps:
  - name: "Analyze Requirements"
    prompt: |
      I want to add a feature that validates email addresses. 
      Please outline:
      1. Test cases (valid/invalid formats)
      2. Edge cases (international domains, subdomains)
      3. Recommended library or algorithm
    when: true

  - name: "Write Tests"
    prompt: |
      Based on the analysis above, write comprehensive unit tests for email validation.
      Use Rust's standard testing framework.
      Don't implement yet — just the tests.
    when: true

  - name: "Implement Feature"
    prompt: |
      Now implement the email validation function to make the tests pass.
      Keep it simple and well-commented.
    when: true

  - name: "Review for Edge Cases"
    prompt: |
      Review the implementation. Did we handle all the edge cases from the analysis?
      Suggest any additions.
    when: true

  - name: "Optimize"
    prompt: |
      The implementation works. Can we make it faster or more readable?
      Suggest optimizations without breaking tests.
    when: true

test_cmd: "cargo test"  # Verify all tests pass
```

## Conditional Steps (when)

Control which steps run using the `when` field:

```yaml
steps:
  - name: "Check Compilation"
    prompt: "Does the code compile? Run `cargo check`"

  - name: "Fix Compilation Errors"
    prompt: "Fix any compilation errors"
    when: false  # conditionally enable based on previous step
    # In future versions: when: "previous.step.failed"

  - name: "Run Tests"
    prompt: "Run the test suite with `cargo test`"
    when: true  # always run
```

Currently, `when` accepts boolean literals. Future versions will support expressions like:
- `when: "previous_step.success"`
- `when: "output_contains('error')"`

## TDD Hook: test_cmd

The `test_cmd` field runs a shell command after all steps complete. If it fails, the recipe reports failure.

```yaml
name: "Implementation Recipe"

steps:
  - name: "Write Implementation"
    prompt: "Implement the feature"

  - name: "Add Tests"
    prompt: "Write tests for the feature"

test_cmd: "cargo test && cargo clippy"  # Both must pass
```

Output:

```
Recipe: Implementation Recipe
Step 1: Write Implementation ✓
Step 2: Add Tests ✓
Test command: cargo test && cargo clippy
Test output:
  running 5 tests
  test result: ok. 5 passed; 0 failed; 0 ignored
  Finished release [optimized] target(s) in 2.34s
Recipe result: SUCCESS
```

If `test_cmd` fails:

```
Recipe result: FAILED (test command exited with code 1)
Error output: thread 'main' panicked at 'expected 42, got 41'
```

## Running Recipes

### Interactive Mode

```bash
hone --recipe code-review.yaml
```

Hone runs each step, streaming output. Press `y` at each step to proceed, `n` to skip, or `q` to quit.

```
Recipe: Code Review
Step 1/3: Analyze Changes
Agent: I found 3 issues...
[pause] Continue to next step? [y/n/q]
> y

Step 2/3: Suggest Improvements
Agent: Top improvements are...
[pause] Continue to next step? [y/n/q]
> y

Step 3/3: Generate Summary
Agent: Summary of review...
[pause] Test command: cargo test
✓ All tests passed
Recipe completed successfully.
```

### Batch Mode

```bash
hone --recipe code-review.yaml --non-interactive
```

Runs all steps without pausing. Useful for CI/CD.

### With Model Override

```bash
hone --recipe code-review.yaml --model ollama:llama2
```

Use a specific model for all steps in the recipe.

### With Budget Limit

```bash
hone --recipe code-review.yaml --budget 5.00
```

Stop if the recipe exceeds $5 in API costs.

## Recipe Output and Export

Each recipe run produces a report:

```bash
hone --recipe code-review.yaml --export-to review-report.json
```

Report format:

```json
{
  "recipe_name": "Code Review",
  "started_at": "2026-04-05T14:30:00Z",
  "completed_at": "2026-04-05T14:32:15Z",
  "steps": [
    {
      "name": "Analyze Changes",
      "output": "The pull request contains...",
      "skipped": false,
      "tokens_input": 1200,
      "tokens_output": 340,
      "cost_usd": 0.024
    },
    {
      "name": "Suggest Improvements",
      "output": "Top 3 improvements: ...",
      "skipped": false,
      "tokens_input": 2100,
      "tokens_output": 520,
      "cost_usd": 0.035
    }
  ],
  "test_result": {
    "cmd": "cargo test",
    "status": "passed",
    "output": "test result: ok. 5 passed"
  },
  "session_cost_usd": 0.059,
  "status": "success"
}
```

## Recipe Directory and Discovery

Hone looks for recipes in:

1. Current directory (`./*.yaml`)
2. `~/.config/hone/recipes/` (user recipes)
3. `/etc/hone/recipes/` (system recipes)
4. Built-in recipes (bundled with Hone)

List all available recipes:

```bash
hone --list-recipes
```

Output:

```
Available recipes:
  user:
    code-review.yaml — Review a code change and suggest improvements
    tdd-feature.yaml — Implement a feature test-first
  system:
    documentation.yaml — Auto-generate API documentation
    refactor.yaml — Refactor code safely
  built-in:
    unit-test.yaml — Write comprehensive unit tests
    integration-test.yaml — Write integration tests
```

## Writing Your Own Recipe

Step-by-step:

1. Create a `.yaml` file:

```bash
cat > my-recipe.yaml << 'EOF'
name: "My Workflow"
description: "What this recipe does"

steps:
  - name: "Step 1"
    prompt: "First prompt"
  
  - name: "Step 2"
    prompt: "Second prompt"

test_cmd: "echo 'All done!'"
EOF
```

2. Test it locally:

```bash
hone --recipe my-recipe.yaml
```

3. Share it: Commit to your repo or publish to the Hone marketplace.

## Best Practices

### 1. Clear Prompts

Write prompts that give the agent specific direction:

```yaml
# Good
prompt: |
  Write unit tests for the `validate_email` function.
  Use Rust's #[test] macro.
  Include at least 5 test cases.

# Vague
prompt: "Write tests"
```

### 2. Break Down Steps

Separate concerns into distinct steps:

```yaml
# Good: each step has a clear focus
steps:
  - name: "Analyze"
    prompt: "Analyze the requirements"
  - name: "Design"
    prompt: "Design a solution"
  - name: "Implement"
    prompt: "Implement the design"

# Vague: too much in one step
steps:
  - name: "Do everything"
    prompt: "Analyze, design, and implement"
```

### 3. Reference Previous Steps

Prompts can reference outputs from earlier steps implicitly (the conversation history includes them):

```yaml
steps:
  - name: "Identify Issues"
    prompt: "List all issues in the code"
  
  - name: "Fix Issues"
    prompt: "Fix the issues you identified above"
    # The agent sees the previous step's output in context
```

### 4. Use test_cmd for Verification

Always include a test command to catch regressions:

```yaml
test_cmd: "cargo test && cargo clippy && cargo fmt --check"
```

## Marketplace

Share recipes with the community (future feature):

```bash
hone recipe-publish my-recipe.yaml
```

This publishes your recipe to the Hone marketplace. Others can then:

```bash
hone --recipe @username/my-recipe.yaml
```

## Next Steps

- **Getting started:** See [Getting Started](getting-started.md) to run your first session.
- **Providers:** Configure multiple providers in [Providers Guide](providers.md).
- **Tools:** Use recipes with custom tools in [Tools Guide](tools-and-mcp.md).
- **Server:** Trigger recipes remotely via [Server Guide](server-and-remote.md).
