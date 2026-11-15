# Recipes: Automating Workflows with YAML

Recipes are YAML files that define multi-step agent workflows. Each step runs a prompt through the model, and steps can depend on previous outputs.

## Built-in Recipes

Hone includes 4 bundled recipes for common development tasks:

### tdd-feature

Implement a feature using Test-Driven Development (Red-Green-Refactor):

```yaml
name: tdd-feature
description: Implement a feature using Test-Driven Development
steps:
  - name: understand
    prompt: "Read the relevant code and understand the current architecture. Summarize what you find."
  - name: red
    prompt: "Write failing tests that describe the desired behavior. Run them to confirm they fail."
  - name: green
    prompt: "Write the minimum code to make the tests pass. Do not add extra features."
  - name: refactor
    prompt: "Refactor the implementation for clarity and quality. All tests must still pass."
  - name: verify
    prompt: "Run all tests one final time. Report the results."
test_cmd: "cargo test"
```

### bug-fix

Fix a bug using systematic debugging with regression test:

```yaml
name: bug-fix
description: Fix a bug using systematic debugging with regression test
steps:
  - name: reproduce
    prompt: "Read the bug report and reproduce the issue. Identify the failing behavior."
  - name: regression-test
    prompt: "Write a regression test that captures the bug. Verify it fails."
  - name: fix
    prompt: "Write the minimal fix. Do not refactor surrounding code."
  - name: verify
    prompt: "Run all tests. The regression test should now pass along with all existing tests."
test_cmd: "cargo test"
```

### code-review

Perform a comprehensive code review:

```yaml
name: code-review
description: Perform a comprehensive code review
steps:
  - name: overview
    prompt: "Read the changed files. Understand the intent and scope of the changes."
  - name: correctness
    prompt: "Check for logical errors, edge cases, and off-by-one bugs."
  - name: security
    prompt: "Scan for injection vulnerabilities, secret leakage, and auth issues."
  - name: performance
    prompt: "Identify unnecessary allocations, N+1 queries, and hot-path issues."
  - name: report
    prompt: "Summarize findings in a table sorted by severity (CRITICAL > HIGH > MEDIUM > LOW > INFO)."
```

### refactor

Safely refactor code with test verification at each step:

```yaml
name: refactor
description: Safely refactor code with test verification at each step
steps:
  - name: baseline
    prompt: "Run all existing tests to establish a green baseline. Report results."
  - name: plan
    prompt: "Analyze the code and create a refactoring plan. List each change and its purpose."
  - name: execute
    prompt: "Apply the refactoring changes one at a time. Run tests after each change."
  - name: verify
    prompt: "Run the full test suite. All tests must pass. Report before/after code metrics."
test_cmd: "cargo test"
```

### Using Built-in Recipes

```bash
# List all available recipes
hone --list-recipes

# Run a built-in recipe by name
hone --recipe tdd-feature
hone --recipe bug-fix
hone --recipe code-review
hone --recipe refactor

# Recipes run every step without pausing, so the same command works in CI/CD

# Set the feature and the test command (tdd-feature's steps use {{feature_name}} and {{test_command}})
hone --recipe tdd-feature --var feature_name="CSV export" --var test_command="pytest tests/"
```

The `--list-recipes` command shows all available recipes with descriptions:

```
Available recipes:
  tdd-feature           Implement a feature using Test-Driven Development
  bug-fix               Fix a bug using systematic debugging with regression test
  code-review           Perform a comprehensive code review
  refactor              Safely refactor code with test verification at each step
```

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

## TDD Hook: test_cmd and Test Verification

The `test_cmd` field automatically runs tests after each step and feeds test failures back to the agent for fixing. This creates a tight TDD feedback loop.

```yaml
name: "Implementation Recipe"

steps:
  - name: "Write Implementation"
    prompt: "Implement the feature"

  - name: "Add Tests"
    prompt: "Write tests for the feature"

test_cmd: "cargo test && cargo clippy"  # Both must pass after each step
```

**How test verification works:**

1. Agent executes a step (writes code, modifies files)
2. `test_cmd` runs automatically: `cargo test`
3. **If tests pass:** → move to next step
4. **If tests fail:** → feed test output to agent and retry (up to 3 times)
5. Agent sees the failure and fixes code based on error messages

**Example output:**

```
Recipe: TDD Implementation
Step 1: Implement Authentication Function
  Running: cargo test
  test auth::test_login ... FAILED
  error[E0308]: mismatched types
  
  [Retry 1/3] Agent re-runs step with test output as context...
  test auth::test_login ... PASSED ✓
  
Step 2: Add Documentation
  Running: cargo test && cargo clippy
  test result: ok. 5 passed ✓
  clippy: no warnings ✓

Recipe result: SUCCESS
```

**Configuration:**

```yaml
name: "TDD Feature"
description: "Build a feature with automatic test verification"
test_cmd: "pytest tests/"  # Run after each step
steps:
  - name: write_tests
    prompt: "Write failing tests for the new feature"
  
  - name: implement
    prompt: "Implement the feature to pass tests"
    # test_cmd runs here automatically
    # If tests fail, agent retries with failure details
  
  - name: cleanup
    prompt: "Refactor for clarity"
    # test_cmd runs here too
```

**Retry behavior:**

- Default: retry up to 3 times if tests fail
- Each retry includes the previous test output as context
- If max retries exhausted, recipe halts with error
- Successful runs advance to next step immediately

**When to use test verification:**

- Implementing features (TDD: red-green-refactor)
- Bug fixes (regression test then fix)
- Refactoring (tests ensure behavior preserved)
- Complex algorithms (automatic validation)

**Research:** The TestVerificationLoop catches 40% more bugs than self-generated tests (AgentCoder study). The tight coupling between test failures and agent iterations significantly improves code quality.

## Running Recipes

### What a Run Looks Like

```bash
hone --recipe code-review
```

Hone runs each step in order and streams its output; it never pauses for input. When the recipe has a `test_cmd`, Hone runs it after every step, prints `Tests passed.` or `Tests FAILED:` and carries on either way.

```
Running recipe: code-review (6 steps)
  Perform a comprehensive code review with security and performance analysis

[1/6] overview
…the agent's answer…

[2/6] correctness
…

[6/6] report
…

Recipe 'code-review' complete.
```

### Batch Mode

```bash
hone --recipe code-review
```

Every recipe run is a batch run: Hone runs all steps without pausing, so there is no separate flag for CI/CD.

### With Model Override

```bash
hone --recipe code-review --model ollama:llama2
```

Use a specific model for all steps in the recipe.

### With Budget Limit

There is no `--budget` flag. Set the limit in `~/.config/hone/config.toml`:

```toml
[budget]
session_limit_usd = 5.00
```

Once the recipe has spent more than $5 in API costs, each remaining step fails with `budget exceeded` instead of calling the model.

## Recipe Output and Export

A recipe run prints each step's output, its tool calls and the test results to stdout as plain text. There is no `--export-to` flag and no JSON report, and `--json` doesn't apply to `--recipe`. To keep the output, redirect it:

```bash
hone --recipe code-review > review-report.txt 2>&1   # errors go to stderr
```

## Recipe Directory and Discovery

`--recipe` loads only the 9 built-in recipes, by name. It doesn't read recipe files or directories, and any other name (`code-review.yaml` included) fails with `unknown recipe: <name>`.

List all available recipes:

```bash
hone --list-recipes
```

Output:

```
Available recipes:
  tdd-feature          Implement a feature using Test-Driven Development
  bug-fix              Fix a bug with regression test
  code-review          Comprehensive code review (correctness, security, performance)
  refactor             Safely refactor with test verification at each step
  security-audit       Full OWASP Top 10 security audit
  performance          Profile, analyze, and optimize performance
  migration            Migrate between frameworks, libraries, or API versions
  documentation        Generate or update project documentation
  onboarding           Understand a new codebase (architecture, patterns, entry points)
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

2. Test it locally: not yet possible from the CLI. `hone --recipe my-recipe.yaml` fails with `unknown recipe: my-recipe.yaml`, because `--recipe` loads only the built-in names (see [Recipe Directory and Discovery](#recipe-directory-and-discovery)).

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
