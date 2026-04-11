# Hone Use Cases: Practical Tutorials

This guide covers 13 real-world use cases for Hone, from simple bug fixes to complex multi-agent orchestrations. Each use case includes a concrete scenario, a terminal demo showing actual commands and output, and step-by-step explanation.

---

## Table of Contents

1. [Beginner Use Cases](#beginner-use-cases)
   - Fix a Bug
   - Add a Feature
   - Code Review
   - Explain Code

2. [Intermediate Use Cases](#intermediate-use-cases)
   - TDD Workflow
   - Refactor with Tests
   - Multi-file Rename
   - Database Migration

3. [Advanced Use Cases](#advanced-use-cases)
   - Multi-Agent Orchestration
   - Agent Ensemble
   - MCP Extensions (GitHub)
   - Security Audit
   - Debug a Test Failure

4. [Choosing the Right Mode](#choosing-the-right-mode)
5. [Cost Optimization Tips](#cost-optimization-tips)
6. [Session Workflow](#session-workflow)

---

# Beginner Use Cases

## 1. Fix a Bug

**When to use:** You found a bug and can describe it. The agent reads the code, identifies the root cause, applies a fix, and verifies it works.

**Scenario:** Your login function crashes when `email` is `None`. You want Hone to fix it without manually searching the codebase.

**Terminal Demo:**

```
$ hone "The login function crashes when email is None. Fix it."

  ✦ read_file auth.py
  ✦ grep -n "email is None" auth.py
  ✓ Found the issue at line 42: no null check before string operations
  
  ✎ edit_file auth.py
    - def login(email: str, password: str):
    -     user = db.find_user(email)
    + def login(email: str, password: str):
    +     if email is None:
    +         return None
    +     user = db.find_user(email)
  
  ❯ shell python -m pytest tests/test_auth.py::test_login -q
    1 passed in 0.2s

  Fixed. Added null check for email parameter before database lookup.

  deepseek-chat │ $0.001 │ ▓░░░░░░░░░ 12% ctx │ 3s
```

**Step-by-Step Explanation:**

1. **Understand**: Hone reads `auth.py` to understand the login function structure
2. **Search**: Uses grep to find references to the email check (or lack thereof)
3. **Identify**: Recognizes the crash happens because `email` is used without validation
4. **Plan**: Determines minimal fix: add a null check at function entry
5. **Edit**: Edits the file using `edit_file` with the exact old and new strings
6. **Verify**: Runs the test suite to confirm the fix works
7. **Report**: Explains what was changed in one line

**Tips and Variations:**

- **Add logging:** Ask Hone to "fix the crash AND add a debug log message when email is None"
- **Multiple languages:** If you have Python and TypeScript files, Hone will fix both (using LSP for each language)
- **Custom test:** Replace the default test with your own: "Fix the crash. Then run `make test-auth`"

**Cost:** ~$0.001-0.002 (DeepSeek V3)

---

## 2. Add a Feature

**When to use:** You want the agent to create a new function, endpoint, or module based on your specification.

**Scenario:** Add a new `/health` endpoint to your FastAPI app that returns `{"status": "ok", "timestamp": "..."}`.

**Terminal Demo:**

```
$ hone "Add a /health endpoint that returns {status: ok, timestamp: iso8601}"

  ✦ read_file app.py
  ✓ Found FastAPI app at line 12
  
  ✎ edit_file app.py
    + from datetime import datetime
    +
    + @app.get("/health")
    + def health_check():
    +     return {
    +         "status": "ok",
    +         "timestamp": datetime.utcnow().isoformat()
    +     }
  
  ❯ shell curl -s http://localhost:8000/health | jq .
    {
      "status": "ok",
      "timestamp": "2026-04-09T15:30:22.123456"
    }

  Added /health endpoint. Returns 200 OK with status and timestamp.

  deepseek-chat │ $0.001 │ ▓░░░░░░░░░ 12% ctx │ 4s
```

**Step-by-Step Explanation:**

1. **Read the code**: Hone examines the existing FastAPI app to understand the structure
2. **Find the right location**: Identifies where to add the new endpoint (typically near other routes)
3. **Write the code**: Generates the endpoint function with proper imports and formatting
4. **Verify**: Optionally runs a quick test (curl, pytest, etc.) to confirm it works
5. **Report**: Summarizes the change

**Tips and Variations:**

- **Add tests**: "Add a /health endpoint AND write a test for it in tests/test_health.py"
- **Dependency injection**: "Add /health that uses the database connection pool to report db status"
- **Error handling**: "Add /health that returns 503 if database is unreachable"
- **Documentation**: "Add /health endpoint with full OpenAPI docstring"

**Cost:** ~$0.001-0.003 (DeepSeek V3)

---

## 3. Code Review

**When to use:** You want a specialist to review code for security issues, performance, or best practices.

**Scenario:** You have an authentication module in `src/auth.py` and want to check it for security vulnerabilities.

**Terminal Demo:**

```
$ hone --agent code-reviewer "Review src/auth.py for security issues"

  ✦ read_file src/auth.py
  ✦ lsp_diagnostics src/auth.py
  ✦ grep -n "secret\|password\|token" src/auth.py
  
  [code-reviewer] Findings:
  
  CRITICAL: Hardcoded secret at line 8
  - Code: SECRET_KEY = "dev-key-12345"
  - Risk: Secrets in source code exposed in git history
  - Fix: Load from environment variable via os.getenv("SECRET_KEY")
  
  HIGH: Plaintext password comparison at line 34
  - Code: if password == stored_password:
  - Risk: Timing attack vulnerability
  - Fix: Use constant-time comparison: secrets.compare_digest(password, stored_password)
  
  MEDIUM: No CSRF token validation on login form
  - Risk: Cross-site request forgery on login endpoint
  - Fix: Validate request.form['csrf_token'] against session
  
  Low-risk findings: 2 (see full report below)

  deepseek-chat │ $0.002 │ ▓▓░░░░░░░░ 16% ctx │ 8s
```

**Step-by-Step Explanation:**

1. **Read the code**: Agent reads the authentication module
2. **Run diagnostics**: Checks for type errors, imports, and common patterns
3. **Search for patterns**: Greps for security-sensitive keywords (secret, password, token)
4. **Analyze**: Applies security knowledge to identify vulnerabilities
5. **Report**: Lists findings by severity with explanations and fixes

**Tips and Variations:**

- **Performance review**: Use `--agent performance-optimizer` instead
- **Automatically fix**: "Review src/auth.py for security issues AND fix the critical ones"
- **Specific focus**: "Review src/auth.py focusing on password handling and token validation"

**Cost:** ~$0.001-0.003 (DeepSeek V3)

---

## 4. Explain Code

**When to use:** You encounter unfamiliar code and want the agent to walk you through what it does.

**Scenario:** You inherit a codebase with a complex rate limiter and want to understand how it works.

**Terminal Demo:**

```
$ hone "Explain what src/middleware/rate_limiter.rs does in detail"

  ✦ read_file src/middleware/rate_limiter.rs
  ✦ grep -n "struct\|impl\|fn " src/middleware/rate_limiter.rs
  
  The rate limiter implements a **sliding window counter** for per-IP request throttling.
  
  Key components:
  
  1. **RateLimiter struct** (lines 10-15)
     Stores: window_size_ms, max_requests, per_ip_counters (HashMap)
     Manages concurrent access via Arc<Mutex<>>
  
  2. **RateLimiter::new()** (lines 17-22)
     Constructor. Initializes empty counters map.
  
  3. **RateLimiter::check_limit()** (lines 24-45)
     Core logic:
     a. Get current timestamp
     b. For the requesting IP, find all requests in [now - window, now]
     c. If count < max_requests, allow and record timestamp
     d. Else, reject with 429 Too Many Requests
  
  4. **Cleanup** (lines 47-50)
     Removes old entries to prevent HashMap growth
  
  Time complexity: O(requests_per_window) per check
  Space complexity: O(unique_ips * requests_per_window)
  
  Common usage:
  ```
  let limiter = RateLimiter::new(window_ms: 60000, max: 100);
  match limiter.check_limit(&ip) {
      Ok(()) => next_handler(),
      Err(_) => 429_response(),
  }
  ```

  deepseek-chat │ $0.002 │ ▓▓░░░░░░░░ 16% ctx │ 5s
```

**Step-by-Step Explanation:**

1. **Read the code**: Agent examines the file
2. **Extract structure**: Greps for key definitions (structs, functions, types)
3. **Understand flow**: Traces the logic from initialization through execution
4. **Explain patterns**: Identifies algorithms (sliding window, hash table, etc.)
5. **Provide context**: Gives time/space complexity and typical usage

**Tips and Variations:**

- **Visual diagram**: "Explain src/rate_limiter.rs AND draw a diagram showing the data flow"
- **Comparison**: "Explain how src/rate_limiter.rs compares to a token bucket approach"
- **Specific function**: "Explain the check_limit() function in detail"
- **With examples**: "Explain src/rate_limiter.rs with 2-3 code examples"

**Cost:** ~$0.001-0.002 (DeepSeek V3)

---

# Intermediate Use Cases

## 5. TDD Workflow

**When to use:** You want to implement a feature using Test-Driven Development: write test first (Red), then implementation (Green), then refactor (Refactor).

**Scenario:** Add rate limiting to an API. You want the structure enforced: test first, implementation, refactoring, verification.

**Terminal Demo:**

```
$ hone --recipe tdd-feature

Hone TDD Recipe
===============

What feature do you want to implement? 
> Add rate limiting to the /api/data endpoint (max 10 requests per minute per IP)

Step 1 of 4: Write the test [RED]
========================================

✎ write_file tests/test_rate_limit.py
   + def test_rate_limit_enforced():
   +     client = TestClient(app)
   +     ip = "192.168.1.1"
   +     
   +     # First 10 requests should succeed
   +     for i in range(10):
   +         resp = client.get("/api/data", headers={"X-Forwarded-For": ip})
   +         assert resp.status_code == 200, f"Request {i+1} should succeed"
   +     
   +     # 11th request should be rate limited
   +     resp = client.get("/api/data", headers={"X-Forwarded-For": ip})
   +     assert resp.status_code == 429, "Request 11 should be rate limited"

❯ shell pytest tests/test_rate_limit.py -q
  FAILED tests/test_rate_limit.py::test_rate_limit_enforced - AttributeError: 'app' has no rate limiter

✓ TEST FAILED (expected)


Step 2 of 4: Implement the feature [GREEN]
===========================================

✎ edit_file app.py
   + from ratelimit import RateLimiter
   + 
   + limiter = RateLimiter(max_requests=10, window_seconds=60)
   + 
   + @app.get("/api/data")
   + async def get_data(request: Request):
   +     if not limiter.check(request.client.host):
   +         raise HTTPException(status_code=429, detail="Rate limited")
   +     return {"data": [...]}

❯ shell pytest tests/test_rate_limit.py -q
  1 passed in 0.3s

✓ TEST PASSED


Step 3 of 4: Refactor for quality [REFACTOR]
==============================================

✎ edit_file app.py
   - limiter = RateLimiter(max_requests=10, window_seconds=60)
   + limiter = RateLimiter(
   +     max_requests=10,
   +     window_seconds=60,
   +     key_func=lambda r: r.headers.get("X-Forwarded-For", r.client.host)
   + )

❯ shell pytest tests/test_rate_limit.py -q
  1 passed in 0.3s

✓ TESTS STILL PASSING


Step 4 of 4: Verify full test suite [VERIFY]
=============================================

❯ shell pytest tests/ -q
  15 passed in 1.2s

✓ ALL TESTS PASS

TDD workflow complete. Feature implemented with test coverage.

deepseek-chat │ $0.005 │ ▓▓▓░░░░░░░ 24% ctx │ 18s
```

**Step-by-Step Explanation:**

1. **RED phase**: Agent writes a failing test that specifies the desired behavior
2. **GREEN phase**: Agent implements the minimal code to make the test pass
3. **REFACTOR phase**: Agent improves code quality without changing behavior
4. **VERIFY phase**: Agent runs the full test suite to ensure no regressions

**Tips and Variations:**

- **Skip refactor**: "Use tdd-feature but skip the refactor phase"
- **Multiple tests**: "Implement feature X with at least 5 test cases"
- **Integration test**: Include end-to-end tests alongside unit tests
- **With documentation**: "After TDD, add API documentation to the endpoint"

**Cost:** ~$0.003-0.005 (DeepSeek V3)

---

## 6. Refactor with Tests

**When to use:** You want to safely refactor code while ensuring tests still pass. The agent makes incremental changes and verifies at each step.

**Scenario:** You want to refactor a User class to use Python dataclasses instead of manual `__init__` and `__repr__`.

**Terminal Demo:**

```
$ hone "Refactor the User class to use dataclasses. Run tests after each change."

  ✦ read_file models.py
  ✓ Found User class at lines 10-35

  [refactor] Step 1: Add dataclasses import
  ✎ edit_file models.py
    + from dataclasses import dataclass
    - class User:
    + @dataclass
    + class User:

  ❯ shell pytest tests/test_models.py::UserTests -q
    3 passed in 0.2s
  ✓ Tests pass after step 1

  [refactor] Step 2: Convert __init__ to fields
  ✎ edit_file models.py
    - def __init__(self, email, password_hash, created_at):
    -     self.email = email
    -     self.password_hash = password_hash
    -     self.created_at = created_at
    + email: str
    + password_hash: str
    + created_at: datetime

  ❯ shell pytest tests/test_models.py::UserTests -q
    3 passed in 0.2s
  ✓ Tests pass after step 2

  [refactor] Step 3: Remove redundant __repr__
  ✎ edit_file models.py
    - def __repr__(self):
    -     return f"<User {self.email}>"
    (removed - dataclass generates this automatically)

  ❯ shell pytest tests/test_models.py -q
    12 passed in 0.3s
  ✓ All tests pass

  Refactored User class from manual implementation to @dataclass.
  Benefits: 15 fewer lines, auto-generated __repr__ and __eq__, type hints.

  deepseek-chat │ $0.003 │ ▓▓░░░░░░░░ 20% ctx │ 9s
```

**Step-by-Step Explanation:**

1. **Understand original**: Reads the current User class implementation
2. **Plan incremental changes**: Identifies safe, minimal refactoring steps
3. **Apply step 1**: Adds decorator and imports
4. **Test**: Runs test suite to confirm no breakage
5. **Apply step 2-N**: Continues incrementally, testing after each change
6. **Verify**: Final full test suite run

**Tips and Variations:**

- **Extract method**: "Refactor the payment logic into a separate PaymentProcessor class"
- **Rename safely**: "Rename all instances of 'total_amount' to 'grand_total' (use LSP for correctness)"
- **Dependency injection**: "Refactor the service layer to use dependency injection instead of globals"
- **Type migration**: "Add type hints to the User class (Python 3.8+)"

**Cost:** ~$0.002-0.004 (DeepSeek V3)

---

## 7. Multi-file Rename

**When to use:** You need to rename something (variable, function, class) across multiple files. Using real LSP ensures correctness (not just string matching).

**Scenario:** Rename the function `calculate_total` to `compute_total` everywhere in the codebase.

**Terminal Demo:**

```
$ hone "Rename the function calculate_total to compute_total everywhere"

  ✦ symbol_search "calculate_total"
  ✓ Found 23 references across 5 files:
    - src/billing.py: 3 definitions, 5 usages
    - src/reports.py: 2 usages
    - tests/test_billing.py: 4 usages
    - docs/API.md: 1 mention
    - src/legacy.py: 8 usages

  [multi-edit] Preparing atomic rename using LSP
  
  ✎ multi_edit
    src/billing.py:
      - def calculate_total(items: List[Item]) -> float:
      + def compute_total(items: List[Item]) -> float:
      - return calculate_total(updated_items)
      + return compute_total(updated_items)
      (5 more replacements...)
    
    src/reports.py:
      - total = calculate_total(yearly_transactions)
      + total = compute_total(yearly_transactions)
      (1 more replacement...)
    
    tests/test_billing.py:
      - assert calculate_total([...]) == 150.00
      + assert compute_total([...]) == 150.00
      (3 more replacements...)
    
    docs/API.md:
      - ## calculate_total()
      + ## compute_total()

  ❯ shell pytest tests/ -q
    18 passed in 0.5s
  ✓ All tests pass after rename

  ❯ lsp_diagnostics
  ✓ No type errors

  Renamed calculate_total -> compute_total across 23 references in 5 files.
  Tests and type checking pass.

  deepseek-chat │ $0.002 │ ▓░░░░░░░░░ 14% ctx │ 6s
```

**Step-by-Step Explanation:**

1. **Search**: Uses symbol_search (LSP-based) to find all references
2. **Plan**: Identifies all files and contexts where the rename must happen
3. **Atomic edit**: Uses `multi_edit` to apply all changes atomically
4. **Verify**: Runs tests and LSP diagnostics to confirm correctness
5. **Report**: Shows number of files and references changed

**Tips and Variations:**

- **Class rename**: "Rename the class UserAccount to CustomerProfile everywhere"
- **Const rename**: "Rename MAX_RETRIES to MAX_RETRY_ATTEMPTS"
- **Import update**: "Rename the module from src/utils.py to src/helpers.py and update all imports"
- **Type rename**: "Rename struct User to Person in Rust code"

**Cost:** ~$0.001-0.003 (DeepSeek V3)

---

## 8. Database Migration

**When to use:** You need to create a database schema change (add/drop/modify columns) with a migration file.

**Scenario:** Add a `last_login` timestamp column to the users table in PostgreSQL.

**Terminal Demo:**

```
$ hone "Add a 'last_login' timestamp column to the users table"

  ✦ read_file alembic/versions/
  ✓ Found latest migration at 001_initial_schema.py
  
  ✎ write_file alembic/versions/002_add_last_login.py
   + from alembic import op
   + import sqlalchemy as sa
   + 
   + revision = '002_add_last_login'
   + down_revision = '001_initial_schema'
   + 
   + def upgrade():
   +     op.add_column('users',
   +         sa.Column('last_login', sa.DateTime, nullable=True, index=True)
   +     )
   + 
   + def downgrade():
   +     op.drop_column('users', 'last_login')

  ✎ edit_file models.py
    - class User(Base):
    -     email: str
    + class User(Base):
    +     email: str
    +     last_login: Optional[datetime] = None

  ❯ shell alembic upgrade head
    [alembic] Running upgrade 001 -> 002

  ❯ shell psql $DATABASE_URL -c \
      "SELECT column_name FROM information_schema.columns WHERE table_name='users'"
    email
    password_hash
    created_at
    last_login

  ✓ Column added successfully

  Added last_login timestamp column to users table with index for query performance.

  deepseek-chat │ $0.003 │ ▓▓░░░░░░░░ 18% ctx │ 7s
```

**Step-by-Step Explanation:**

1. **Read schema**: Reads existing migration files to understand the pattern
2. **Create migration**: Generates an Alembic migration file (up and down)
3. **Update model**: Updates the SQLAlchemy model to match the schema
4. **Test migration**: Runs `alembic upgrade` to apply the change
5. **Verify**: Confirms the column exists in the database

**Tips and Variations:**

- **Data migration**: "Add a status column to orders (with default='pending') and backfill existing rows"
- **Index addition**: "Add an index on the email column for faster lookups"
- **Constraint**: "Add a UNIQUE constraint on email"
- **Rollback test**: "Create a migration to add the column AND test that the down() rollback works"

**Cost:** ~$0.002-0.004 (DeepSeek V3)

---

# Advanced Use Cases

## 9. Multi-Agent Orchestration

**When to use:** You have a complex task that benefits from decomposition. The orchestrator breaks it into subtasks, workers execute them in parallel, and results are merged.

**Scenario:** Implement user authentication with JWT tokens. This involves: backend token generation, frontend integration, token refresh logic, and security review.

**Terminal Demo:**

```
$ hone --orchestrate "Implement user authentication with JWT tokens"

  [orchestrator] Decomposing task...
  
  Subtasks:
  1. auth-backend: Implement JWT generation and validation in Python
  2. auth-frontend: Add login form and token storage in TypeScript
  3. refresh-logic: Implement token refresh with sliding expiration
  4. security-review: Audit the implementation for vulnerabilities

  [orchestrator] Spawning 4 workers...
  =====================================

  [worker 1 - auth-backend] Running
  ✦ read_file app.py
  ✎ write_file auth.py
   + import jwt
   + from datetime import datetime, timedelta
   + 
   + SECRET_KEY = os.getenv("JWT_SECRET")
   + TOKEN_EXPIRY_HOURS = 1
   + 
   + def generate_token(user_id: str) -> str:
   +     payload = {
   +         "user_id": user_id,
   +         "exp": datetime.utcnow() + timedelta(hours=TOKEN_EXPIRY_HOURS)
   +     }
   +     return jwt.encode(payload, SECRET_KEY, algorithm="HS256")
   + 
   + def verify_token(token: str) -> Optional[str]:
   +     try:
   +         payload = jwt.decode(token, SECRET_KEY, algorithms=["HS256"])
   +         return payload["user_id"]
   +     except jwt.ExpiredSignatureError:
   +         return None
  
  [worker 2 - auth-frontend] Running
  ✎ write_file src/components/LoginForm.tsx
   + export function LoginForm() {
   +     const [email, setEmail] = useState("");
   +     const [password, setPassword] = useState("");
   +     
   +     const handleLogin = async () => {
   +         const res = await fetch("/api/login", {
   +             method: "POST",
   +             body: JSON.stringify({ email, password })
   +         });
   +         const { token } = await res.json();
   +         localStorage.setItem("auth_token", token);
   +     };
   +     
   +     return (
   +         <form onSubmit={handleLogin}>
   +             <input value={email} onChange={e => setEmail(e.target.value)} />
   +             <input type="password" value={password} onChange={e => setPassword(e.target.value)} />
   +             <button type="submit">Login</button>
   +         </form>
   +     );
   + }
  
  [worker 3 - refresh-logic] Running
  ✎ edit_file auth.py
   + REFRESH_TOKEN_EXPIRY_DAYS = 7
   + 
   + def generate_refresh_token(user_id: str) -> str:
   +     payload = {
   +         "user_id": user_id,
   +         "type": "refresh",
   +         "exp": datetime.utcnow() + timedelta(days=REFRESH_TOKEN_EXPIRY_DAYS)
   +     }
   +     return jwt.encode(payload, SECRET_KEY, algorithm="HS256")
   + 
   + @app.post("/api/refresh")
   + def refresh_token_endpoint(refresh_token: str):
   +     user_id = verify_token(refresh_token)
   +     if not user_id:
   +         raise HTTPException(status_code=401)
   +     return {"token": generate_token(user_id)}
  
  [worker 4 - security-review] Running
  ✦ read_file auth.py
  ✓ CRITICAL: TOKEN NOT VALIDATED ON EVERY REQUEST
    Fix: Add middleware to check Authorization: Bearer <token>
  ✓ HIGH: REFRESH TOKEN SAME ALGORITHM AS ACCESS TOKEN
    Risk: Attacker can use refresh token as access token
    Fix: Sign refresh token with different key or add "type" claim
  ✓ MEDIUM: NO LOGOUT ENDPOINT
    Risk: Tokens cannot be revoked early
    Fix: Add token blacklist (Redis set)

  [orchestrator] Merging results...
  
  ✎ edit_file app.py
   + from functools import wraps
   + 
   + def require_auth(f):
   +     @wraps(f)
   +     def wrapper(*args, **kwargs):
   +         token = request.headers.get("Authorization", "").replace("Bearer ", "")
   +         user_id = verify_token(token)
   +         if not user_id:
   +             raise HTTPException(status_code=401)
   +         return f(*args, user_id=user_id, **kwargs)
   +     return wrapper
   + 
   + @app.get("/api/protected")
   + @require_auth
   + def protected_endpoint(user_id: str):
   +     return {"user_id": user_id}
  
  ❯ shell pytest tests/test_auth.py -q
    12 passed in 0.8s

  Authentication system implemented:
  - Backend: JWT generation and validation with 1-hour expiry
  - Frontend: Login form stores token in localStorage
  - Refresh: Sliding token refresh with 7-day refresh token
  - Security: Middleware validates tokens on protected endpoints, separate refresh token key

  deepseek-chat │ $0.012 │ ▓▓▓▓░░░░░░ 32% ctx │ 45s
```

**Step-by-Step Explanation:**

1. **Orchestrator reads the task**: Understands the requirements
2. **Decompose**: Breaks into 4 logical subtasks (backend, frontend, refresh, security)
3. **Spawn workers**: 4 agents run in parallel on their subtasks
4. **Merge results**: Lead agent integrates results, applies security fixes, runs tests
5. **Report**: Summarizes what was implemented

**Tips and Variations:**

- **Longer orchestrations**: "Build a complete user management system: auth, profiles, RBAC, audit logging"
- **Multi-language**: "Implement API pagination in Go backend and React frontend"
- **Cross-cutting**: "Add comprehensive logging to all services"

**Cost:** ~$0.010-0.015 (4 workers running in parallel, DeepSeek V3)

---

## 10. Agent Ensemble

**When to use:** You want multiple AI candidates to solve the same problem, ranked for quality. Useful for complex algorithmic tasks.

**Scenario:** Write a Rust function that finds the longest palindromic substring in O(n) time using Manacher's algorithm.

**Terminal Demo:**

```
$ hone --ensemble 3 "Write a Rust function to find the longest palindromic substring using Manacher's algorithm. Include tests."

  [ensemble] Running 3 candidates in parallel...
  ==============================================

  [candidate 1: temperature=0.3 - deterministic]
  ✓ Generated 45 lines of code, 3 test cases
  
  [candidate 2: temperature=0.7 - balanced]
  ✓ Generated 52 lines of code, 5 test cases
  
  [candidate 3: temperature=1.0 - creative]
  ✓ Generated 61 lines of code, 4 test cases

  [ranker] Analyzing candidates...
  
  Candidate 1: SCORE 0.82
    Strengths: Correct algorithm, clear structure
    Weaknesses: Missing edge case tests for even-length palindromes
  
  Candidate 2: SCORE 0.91
    Strengths: Most complete test coverage, handles even/odd palindromes, includes boundary tests
    Weaknesses: Slightly verbose with comments
  
  Candidate 3: SCORE 0.79
    Strengths: Creative error handling
    Weaknesses: Incomplete test coverage, unused variable in main logic

  [ranker] SELECTED: Candidate 2 (score 0.91)
  Reason: Most comprehensive test coverage with proper handling of both even and odd-length palindromes.

  ✨ Using solution from Candidate 2:

  ✎ write_file src/palindrome.rs
   + pub fn longest_palindrome(s: &str) -> String {
   +     if s.is_empty() { return String::new(); }
   +     
   +     let chars: Vec<char> = s.chars().collect();
   +     let n = chars.len();
   +     let mut expanded = vec!['|'];
   +     
   +     for c in &chars {
   +         expanded.push(*c);
   +         expanded.push('|');
   +     }
   +     
   +     let n_exp = expanded.len();
   +     let mut palindrome_radii = vec![0; n_exp];
   +     let mut center = 0;
   +     let mut right = 0;
   +     
   +     for i in 1..n_exp {
   +         let mirror = 2 * center - i;
   +         
   +         if i < right {
   +             palindrome_radii[i] = std::cmp::min(right - i, palindrome_radii[mirror]);
   +         }
   +         
   +         while i + palindrome_radii[i] + 1 < n_exp
   +             && i - palindrome_radii[i] - 1 >= 0
   +             && expanded[i + palindrome_radii[i] + 1] == expanded[i - palindrome_radii[i] - 1]
   +         {
   +             palindrome_radii[i] += 1;
   +         }
   +         
   +         if i + palindrome_radii[i] > right {
   +             center = i;
   +             right = i + palindrome_radii[i];
   +         }
   +     }
   +     
   +     let max_len = *palindrome_radii.iter().max().unwrap_or(&0);
   +     let center_max = palindrome_radii.iter().position(|&x| x == max_len).unwrap();
   +     
   +     let start = (center_max - max_len) / 2;
   +     chars[start..start + max_len].iter().collect()
   + }
   + 
   + #[cfg(test)]
   + mod tests {
   +     use super::*;
   +     
   +     #[test]
   +     fn test_odd_palindrome() {
   +         assert_eq!(longest_palindrome("racecar"), "racecar");
   +     }
   +     
   +     #[test]
   +     fn test_even_palindrome() {
   +         assert_eq!(longest_palindrome("abccba"), "abccba");
   +     }
   +     
   +     #[test]
   +     fn test_single_char() {
   +         assert_eq!(longest_palindrome("a"), "a");
   +     }
   +     
   +     #[test]
   +     fn test_no_palindrome() {
   +         let result = longest_palindrome("abcd");
   +         assert_eq!(result.len(), 1);
   +     }
   +     
   +     #[test]
   +     fn test_entire_string() {
   +         assert_eq!(longest_palindrome("aabbaa"), "aabbaa");
   +     }
   + }

  ❯ shell cargo test --lib palindrome -q
    running 5 tests
    test palindrome::tests::test_odd_palindrome ... ok
    test palindrome::tests::test_even_palindrome ... ok
    test palindrome::tests::test_single_char ... ok
    test palindrome::tests::test_no_palindrome ... ok
    test palindrome::tests::test_entire_string ... ok
    
    test result: ok. 5 passed

  ✓ All tests pass. Time complexity: O(n), Space complexity: O(n)

  Best solution selected from 3 candidates using MASAI Ranker (arXiv 2406.11638).

  deepseek-chat │ $0.005 │ ▓▓▓░░░░░░░ 28% ctx │ 22s
```

**Step-by-Step Explanation:**

1. **Generate candidates**: 3 agents run the same prompt at different temperatures (0.3, 0.7, 1.0)
2. **Rank**: A dedicated ranker agent scores each candidate on code quality, test coverage, and correctness
3. **Select**: The best candidate (highest score) is chosen
4. **Verify**: The selected solution is tested to confirm it works
5. **Report**: Shows the scores and ranking rationale

**Tips and Variations:**

- **More candidates**: Use `--ensemble 5` for more diversity (up to 5 supported)
- **Custom ranker prompt**: "Generate 3 solutions for X and rank them on performance, readability, and test coverage"
- **Different models**: Mix models in the ensemble for different "styles" of solutions

**Cost:** ~$0.004-0.008 (3 candidates + ranker, DeepSeek V3)

---

## 11. MCP Extensions: Create a GitHub PR

**When to use:** You want to use MCP servers to extend Hone's capabilities. GitHub MCP allows creating issues, PRs, and reading repos without cloning.

**Scenario:** Your README has a typo. You want Hone to fix it and create a PR automatically.

**Terminal Demo:**

```
$ export GITHUB_TOKEN="ghp_..."
$ hone "Fix the typo in README.md line 42 and create a PR"

  ✦ read_file README.md | head -n 50
  ✓ Found typo: "integrtion" should be "integration"
  
  [mcp-github] Creating branch...
  ✓ Created branch: typo-fix-readme

  ✎ edit_file README.md
    - This tool provides integrtion with multiple providers.
    + This tool provides integration with multiple providers.

  [mcp-github] Committing changes...
  ✓ Committed: Fix typo in README

  [mcp-github] Creating pull request...
  ✓ PR created: https://github.com/honelang/hone/pull/234
  
  Title: Fix typo in README: integrtion -> integration
  Description:
    Fixed a typo on line 42 of README.md
    
    Changes:
    - "integrtion" -> "integration"

  deepseek-chat │ $0.002 │ ▓░░░░░░░░░ 14% ctx │ 8s
```

**Setup Instructions:**

1. Create `.hone/mcp.json`:

```json
{
  "mcpServers": {
    "github": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-github"],
      "env": {
        "GITHUB_PERSONAL_ACCESS_TOKEN": "${GITHUB_TOKEN}"
      }
    }
  }
}
```

2. Set your GitHub token:

```bash
export GITHUB_TOKEN="ghp_..."
hone
```

**Tips and Variations:**

- **Create issue**: "Create a GitHub issue for the performance bug in the parser module"
- **Multiple PRs**: "Fix the 3 typos I found in the docs AND create 3 separate PRs"
- **Add labels**: "Create a PR and label it as 'bug' and 'urgent'"

**Cost:** ~$0.001-0.003 (includes MCP communication)

---

## 12. Security Audit

**When to use:** You want a comprehensive security review using the specialized `security-auditor` agent.

**Scenario:** Full OWASP Top 10 audit of your Flask application.

**Terminal Demo:**

```
$ hone --agent security-auditor "Audit this project for OWASP Top 10 vulnerabilities"

  ✦ read_file app.py
  ✦ read_file models.py
  ✦ grep -n "sql\|query\|request\|password\|secret" app.py
  ✦ lsp_diagnostics
  
  SECURITY AUDIT REPORT
  =====================
  
  [CRITICAL] A1: SQL Injection
  Location: app.py line 42
  Code: db.query(f"SELECT * FROM users WHERE id = {user_id}")
  Risk: User input directly interpolated into SQL query
  CWE: CWE-89
  Fix: Use parameterized queries:
       db.query("SELECT * FROM users WHERE id = ?", [user_id])
  
  [CRITICAL] A2: Broken Authentication
  Location: auth.py line 15
  Code: if password == stored_password:
  Risk: Timing attack vulnerability (non-constant-time comparison)
  CWE: CWE-208
  Fix: import secrets
       if secrets.compare_digest(password, stored_password):
  
  [HIGH] A3: Sensitive Data Exposure
  Location: app.py line 5
  Code: SECRET_KEY = "dev-secret-12345"
  Risk: Secrets in source code
  CWE: CWE-798
  Fix: Load from environment: SECRET_KEY = os.getenv("SECRET_KEY")
  
  [HIGH] A5: Broken Access Control
  Location: app.py line 78
  Code: @app.get("/api/user/<id>")
       def get_user(id):
           return User.query.get(id)  # No permission check!
  Risk: Authenticated users can access any user's data
  CWE: CWE-639
  Fix: Add authorization:
       if user.id != id and not user.is_admin:
           raise PermissionDenied()
  
  [MEDIUM] A6: CSRF Protection
  Location: app.py line 95
  Code: @app.post("/api/settings")
       def update_settings():  # No CSRF token check
  Risk: Form submissions from other sites can modify user settings
  CWE: CWE-352
  Fix: Add CSRF middleware: flask_wtf.csrf.csrf_protect
  
  [LOW] A9: Using Components with Known Vulnerabilities
  Location: requirements.txt
  Code: requests==2.25.1  # Released Jan 2021, now has 3 CVEs
  Fix: Upgrade: requests>=2.31.0
  
  Summary:
  - Critical: 2 (SQL injection, timing attack)
  - High: 2 (hardcoded secrets, missing authz)
  - Medium: 1 (CSRF)
  - Low: 1 (outdated dependency)
  
  Estimated fix time: 30 minutes
  Risk level: HIGH - deploy fixes before production

  deepseek-chat │ $0.005 │ ▓▓▓░░░░░░░ 26% ctx │ 12s
```

**Step-by-Step Explanation:**

1. **Scan code**: Reads all Python/JS files, greps for security keywords
2. **Analyze patterns**: Checks for common vulnerabilities (SQL injection, hardcoded secrets, etc.)
3. **Cross-reference CWE/OWASP**: Maps findings to official vulnerability databases
4. **Suggest fixes**: Provides concrete code examples for each issue
5. **Prioritize**: Ranks by severity and exploitability

**Tips and Variations:**

- **Automatic fixes**: "Audit for security AND fix all HIGH and CRITICAL issues"
- **Compliance**: "Audit for PCI-DSS compliance (credit card handling)"
- **Dependency check**: "Check dependencies for known vulnerabilities using CVE database"

**Cost:** ~$0.003-0.005 (security review is thorough but worth it)

---

## 13. Debug a Test Failure

**When to use:** A test fails and you need systematic debugging. The `debugger` agent uses the scientific method: hypothesis, test, observe, refine.

**Scenario:** The `test_user_creation` test fails with `AttributeError: 'NoneType' object has no attribute 'email'`.

**Terminal Demo:**

```
$ hone --agent debugger "test_user_creation fails with AttributeError: 'NoneType' has no attribute 'email'"

  ✦ read_file tests/test_models.py
  ✓ Found test at line 12

  ❯ shell pytest tests/test_models.py::test_user_creation -v
    FAILED tests/test_models.py::test_user_creation - AttributeError: 'NoneType' object has no attribute 'email'
    
    Traceback:
      File "tests/test_models.py", line 18, in test_user_creation
        assert created_user.email == "test@example.com"
      NoneType has no attribute 'email'

  [debugger] Hypothesis 1: user creation returns None instead of User object
  
  ✦ read_file models.py
  ✓ Found User.create() method at line 42
  
  [debugger] Investigating: Does User.create() always return a User?
  
  ✦ grep -n "def create\|return" models.py
  ✓ Code path analysis:
    - Line 42: def create(email, password):
    - Line 43-45: validation
    - Line 46: db.add(user)
    - Line 47: db.commit()  <-- No return statement here!
    - Line 48: (no explicit return, returns None implicitly)

  [debugger] ROOT CAUSE FOUND: User.create() doesn't return the created user
  
  ✎ edit_file models.py
    - def create(email, password):
    -     user = User(email=email, password=password)
    -     db.add(user)
    -     db.commit()
    + def create(email, password):
    +     user = User(email=email, password=password)
    +     db.add(user)
    +     db.commit()
    +     return user

  ❯ shell pytest tests/test_models.py::test_user_creation -v
    PASSED tests/test_models.py::test_user_creation
  ✓ TEST NOW PASSES

  [debugger] Verification: Are there other similar missing return statements?
  
  ✦ grep -n "def.*create\|def.*update\|def.*delete" models.py
  ✓ Checked 8 other CRUD methods. All return objects. No further issues found.

  Root cause: User.create() was missing a return statement.
  Fix: Added `return user` at line 48.
  Verification: Test now passes. No similar issues in other methods.

  deepseek-chat │ $0.003 │ ▓▓░░░░░░░░ 18% ctx │ 11s
```

**Step-by-Step Explanation:**

1. **Run failing test**: Reproduces the error and reads the full traceback
2. **Read test code**: Understands what the test expects
3. **Read implementation**: Examines the code being tested
4. **Form hypothesis**: "User.create() returns None" based on stack trace
5. **Investigate**: Traces execution path to find the actual cause
6. **Identify root cause**: Missing return statement
7. **Fix**: Adds the return statement
8. **Verify**: Re-runs the test to confirm it passes
9. **Check for similar issues**: Scans for similar bugs in related methods

**Tips and Variations:**

- **Flaky test**: "The test_async_payment test passes sometimes and fails randomly. Debug it."
- **Performance test**: "test_bulk_insert takes 30s but should take <5s. Optimize it."
- **Integration failure**: "test_api_with_database fails. Debug the integration between Flask and SQLAlchemy."

**Cost:** ~$0.002-0.004 (DeepSeek V3)

---

# Choosing the Right Mode

This section helps you pick the best invocation style for your task.

## One-Shot Mode

**Command:** `hone "your prompt"`

**Best for:**
- Simple, scoped tasks (bug fix, add function, small refactor)
- Quick answers (explain code, search codebase)
- Throwaway work (experiments, prototyping)

**Examples:**
```bash
hone "Fix the typo in line 42"
hone "Add a print statement to debug the loop"
hone "Explain what this function does"
```

**Characteristics:**
- Fast (seconds)
- Cheap ($0.001-0.003)
- No session persistence
- Model sees full context at once

---

## Interactive Mode

**Command:** `hone` (no arguments, then type multi-turn prompts)

**Best for:**
- Exploring code iteratively
- Back-and-forth conversation
- Session-based work (resume next day)
- Learning a codebase gradually

**Examples:**
```bash
hone
# Hone: What would you like to do?
# You: Explain the authentication flow
# Hone: [detailed explanation]
# You: Show me the login function
# Hone: [reads and explains]
# You: Fix the timing attack vulnerability
# Hone: [makes fix]
```

**Characteristics:**
- Slower (one turn at a time)
- Moderate cost (accumulates across turns)
- Remembers context across turns
- Session auto-saved

---

## Recipe Mode

**Command:** `hone --recipe tdd-feature`

**Best for:**
- Structured workflows (TDD, bug fix, code review, refactor)
- Team standards (enforce Red-Green-Refactor discipline)
- Reproducible processes
- Guiding less experienced developers

**Examples:**
```bash
hone --recipe tdd-feature           # Implement with tests first
hone --recipe bug-fix               # Fix with systematic debugging
hone --recipe code-review           # Comprehensive review
hone --recipe refactor              # Safe refactoring with verification
```

**Characteristics:**
- Multi-step (enforces structure)
- Moderate-to-high cost (multiple steps)
- Highly guided experience
- Perfect for learning

---

## Orchestration Mode

**Command:** `hone --orchestrate "complex task"`

**Best for:**
- Large, complex tasks that benefit from decomposition
- Multiple independent subtasks
- Tasks where parallel work saves time
- Full-feature implementations

**Examples:**
```bash
hone --orchestrate "Implement JWT auth with refresh tokens and logout"
hone --orchestrate "Build a microservice with REST API, background jobs, and monitoring"
hone --orchestrate "Refactor the monolith into 3 services"
```

**Characteristics:**
- Spawns multiple agents (4-5 typical)
- Higher cost (parallel agents)
- Faster overall (parallelism)
- Best quality (diverse specialists)
- Requires complex task

---

## Ensemble Mode

**Command:** `hone --ensemble 3 "algorithm problem"`

**Best for:**
- Algorithmic problems with multiple valid solutions
- High-quality output where cost is less critical
- Competitive problems (coding challenges, interviews)
- When you want to compare approaches

**Examples:**
```bash
hone --ensemble 3 "Write a binary search tree implementation in Rust"
hone --ensemble 5 "Design a rate limiter using token bucket algorithm"
hone --ensemble 3 "Optimize this SQL query for performance"
```

**Characteristics:**
- Runs N agents in parallel, ranks results
- Moderate-to-high cost (N times + ranker)
- Best for well-defined problems
- Visible quality ranking

---

## Agent Selection

Use specific agents for specialized tasks:

| Agent | Use for | Example |
|-------|---------|---------|
| `code-reviewer` | Code quality, security, maintainability | `hone --agent code-reviewer "Review src/auth.py"` |
| `debugger` | Systematic debugging, test failures | `hone --agent debugger "test_x fails with AttributeError"` |
| `security-auditor` | Vulnerability assessment, compliance | `hone --agent security-auditor "Audit for OWASP Top 10"` |
| `performance-optimizer` | Speed, memory, database optimization | `hone --agent performance-optimizer "Speed up the query"` |
| `tdd-workflow` | Test-driven development | `hone --agent tdd-workflow "Add rate limiting"` |
| `rust-expert` | Rust-specific issues (ownership, async) | `hone --agent rust-expert "Fix lifetime errors"` |
| `python-expert` | Python-specific issues (typing, async) | `hone --agent python-expert "Fix the async handler"` |

---

# Cost Optimization Tips

Hone's cost efficiency comes from:

1. **Model selection**: Cheap for simple tasks, powerful for complex ones
2. **Complexity routing**: Automatic classification reduces wasted tokens
3. **Context optimization**: Smart reordering and compression minimizes input tokens
4. **Parallel execution**: Orchestration/ensemble costs less per-task than sequential

## Cost Breakdown

**Typical costs (DeepSeek V3 via OpenRouter):**

| Task | Model | Cost |
|------|-------|------|
| Simple bug fix | DeepSeek | $0.001 |
| Add a feature | DeepSeek | $0.002 |
| Code review | DeepSeek | $0.003 |
| TDD workflow | DeepSeek | $0.004 |
| Orchestration (4 agents) | DeepSeek | $0.010 |
| Ensemble (3 candidates) | DeepSeek | $0.006 |

**Comparison to competitors:**

| Tool | Same Task | Cost | Notes |
|------|-----------|------|-------|
| Hone (DeepSeek) | Bug fix | $0.001 | Fast, local sandboxing |
| Claude Code (Sonnet) | Bug fix | $0.080 | Higher quality, but 80x more expensive |
| GPT-4 (OpenAI) | Bug fix | $0.050 | More capable, still 50x more expensive |

---

## Strategies for Lower Cost

### 1. Use DeepSeek V3 for Exploration

DeepSeek handles 80% of coding tasks perfectly at 1/50th the cost:

```bash
export OPENAI_API_KEY="sk-..."
export HONE_OPENAI_BASE_URL="https://api.deepseek.com"
export HONE_OPENAI_MODEL="deepseek-coder"
hone "Fix the login bug"  # $0.001
```

### 2. Use Claude for Critical Decisions

Save Claude for high-stakes tasks (security reviews, architecture decisions):

```bash
export ANTHROPIC_API_KEY="sk-ant-..."
hone --model claude-3-sonnet "Audit for security vulnerabilities"  # $0.010
```

### 3. Use Ephemeral Mode to Save Session Storage

Skip session persistence if you don't need to resume:

```bash
hone --ephemeral "Quick refactor"  # Doesn't persist to disk
```

### 4. Use Local Models for Repetitive Tasks

Run `deepseek-coder-v2:16b` locally with Ollama ($0.00 cost):

```bash
ollama pull deepseek-coder-v2:16b
hone configure  # Select Ollama
hone "Refactor this function"  # Free, offline, private
```

### 5. Batch Similar Tasks

Use recipes to combine multiple steps into one invocation:

```bash
hone --recipe bug-fix  # 4 structured steps in one session
# vs
hone "write test"
hone "implement fix"
hone "refactor"
hone "verify"            # 4 separate invocations, higher overhead
```

### 6. Avoid Orchestration for Simple Tasks

Orchestration spawns multiple agents (expensive). Use it only when subtasks are genuinely independent:

```bash
# GOOD: Complex task benefits from decomposition
hone --orchestrate "Implement OAuth2 with email verification and 2FA"

# WASTEFUL: Simple task over-engineered
hone --orchestrate "Add a print statement"  # Just use: hone "Add a print statement"
```

---

## Monitoring Costs

Check cost after each session:

```bash
# In interactive mode
hone
# ... do work ...
/cost

# Session cost: $0.012 (1,847 tokens at $0.00065/1K)
```

Or query past sessions:

```bash
hone session list
hone session cost my-session
```

---

# Session Workflow

Sessions allow you to resume work across days, remember context, and track costs.

## Create and Resume Sessions

### Start a Named Session

```bash
hone --session my-project
```

Hone saves the session after each turn. Resume it later:

```bash
hone session resume my-project
```

### List All Sessions

```bash
hone session list

Sessions:
  my-project          (3 turns, $0.012, last turn 1h ago)
  refactor-auth       (12 turns, $0.045, last turn 2d ago)
  feature-x           (8 turns, $0.034, last turn 5d ago)
```

### Rename a Session

```bash
hone session rename my-project auth-refactor
```

### Delete a Session

```bash
hone session delete my-project
```

### Ephemeral (Throwaway) Sessions

```bash
hone --ephemeral "Quick fix"
# Session NOT saved. Useful for experiments.
```

---

## Session Context and Memory

### What Gets Persisted

Each session stores:
- Conversation history
- File changes made
- Tool calls and outputs
- Cost tracking
- Project-specific patterns learned

### Cross-Session Learning

Hone's adaptive memory learns patterns across sessions:

```bash
# Session 1
hone --session project-a -i "Add tests using pytest"
# Hone learns: this project uses pytest

# Session 2 (next day)
hone --session project-a -i "Add more tests"
# Hone remembers: use pytest here (learned from session 1)
```

---

## Typical Session Workflows

### Day 1: Feature Development

```bash
hone --session feature-auth
# Turn 1: Implement login endpoint
# Turn 2: Add JWT token generation
# Turn 3: Write tests
# Turn 4: Security review (auto-saved)

# Cost: $0.008
```

### Day 2: Bug Fixes

```bash
hone session resume feature-auth
# Turn 5: Fix the timing attack
# Turn 6: Optimize query
# (Hone remembers the project structure from day 1)

# Cost: $0.005 (lower because it remembers context)
```

### Multi-Day Project

```bash
# Week 1: Design and implement
hone --session project-x
# Weeks 2-3: Bug fixes and optimization
hone session resume project-x
# End of month: Final review
hone session resume project-x
```

---

## Privacy and Session Storage

### Default Storage

Sessions are stored in `~/.local/share/hone/sessions/`:

```bash
ls ~/.local/share/hone/sessions/
feature-auth.db
my-project.db
```

SQLite databases are plain text. Sensitive code is readable.

### Encrypted Sessions

Enable encryption at compile time:

```bash
cargo build --features encrypted-memory --release
```

Sessions are then encrypted with AES-256-GCM:

```bash
# Now encrypted with system keychain key
hone --session confidential-work
```

---

## Session Governance (Team Use)

If using the `honed` daemon for team collaboration:

```bash
# Admin
honed start

# Developer 1
hone --remote localhost:3117 --session my-task

# Admin can audit
hone remote cost --user dev1 --session my-task  # See cost breakdown
hone remote audit --user dev1                    # See all actions
```

---

# Summary: Quick Reference

| Task | Command | Cost | Time |
|------|---------|------|------|
| Fix a bug | `hone "bug description"` | $0.001 | 5s |
| Add a feature | `hone "feature description"` | $0.002 | 10s |
| Review code | `hone --agent code-reviewer "path"` | $0.003 | 8s |
| TDD workflow | `hone --recipe tdd-feature` | $0.004 | 20s |
| Complex task | `hone --orchestrate "description"` | $0.010 | 45s |
| Best solution | `hone --ensemble 3 "problem"` | $0.006 | 30s |
| Security audit | `hone --agent security-auditor "path"` | $0.005 | 12s |
| Debug test | `hone --agent debugger "error message"` | $0.003 | 10s |

---

# Next Steps

1. **Pick a use case** from this guide that matches your current task
2. **Run the example command** with your code
3. **Monitor the cost** with `/cost` or `hone session list`
4. **Create a custom recipe** if you have a repeated workflow
5. **Set up MCP extensions** if you need GitHub, databases, or web access
6. **Read the security guide** if you handle sensitive code

For more details, see:
- [Configuration Guide](../configuration.md)
- [Tools & MCP](tools-and-mcp.md)
- [Security Best Practices](security.md)
- [Recipes Guide](recipes.md)
