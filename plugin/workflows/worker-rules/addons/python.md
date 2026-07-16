---
name: python
frameworks: ["python", "django", "fastapi", "flask", "starlette", "celery"]
test_frameworks: ["pytest", "unittest", "hypothesis"]
priority: 50
inject_position: after_base
allowed_tools: []
pre_invoke_gate: null
---

# Testing Rules: Python

## MUST
- Use `pytest` fixtures over `setUp`/`tearDown` for new tests; reserve `django.test.TestCase` only when database transactions or Django's test client are required
- Mark async tests with `@pytest.mark.asyncio` and use `httpx.AsyncClient` for FastAPI/Starlette async route tests
- Scope fixtures explicitly (`function`, `module`, `session`); default to `function` scope to avoid state leakage
- Use `pytest-django`'s `@pytest.mark.django_db` on any test that touches the ORM; never open raw DB connections in test bodies
- Patch at the import site of the consuming module, not the definition site: `mock.patch("myapp.views.requests.get")` not `mock.patch("requests.get")`
- Use `hypothesis` strategies for input-space coverage on pure functions with complex domains (parsers, validators, serializers)
- Isolate Celery tasks with `@pytest.mark.celery(task_always_eager=True)` or call `.apply()` directly; never rely on a live broker in unit tests

## MUST NOT
- Do not share mutable fixture state across tests without `function` scope
- Do not use `unittest.mock.patch` as a decorator on async test functions — use it as an async context manager instead
- Do not call `django.setup()` manually inside test modules; let `pytest-django` handle it
- Do not assert on internal Celery signal internals — test the task return value and side effects only
- Do not use `time.sleep` in async tests; use `asyncio.sleep` or mock the clock

## Edge Case Checklist
- Timezone-aware vs naive datetimes in Django ORM queries
- Celery task retry logic: ensure `max_retries` and `countdown` are exercised
- FastAPI dependency overrides: verify `app.dependency_overrides` is cleared after each test
- Hypothesis shrinking: reproduce failures with the logged `@example` decorator
- `pytest-xdist` parallel safety: confirm fixtures using shared resources use locks or unique identifiers
