---
name: java
frameworks: ["java", "kotlin", "spring", "spring-boot", "quarkus", "micronaut"]
test_frameworks: ["junit", "mockito", "testcontainers"]
---

# Testing Rules: Java

## MUST
- Use `@SpringBootTest` only for integration tests that need the full application context; prefer `@WebMvcTest`, `@DataJpaTest`, or `@Service`-level unit tests for focused tests
- Inject dependencies via constructor injection in production code so tests can instantiate classes directly without Spring context
- Use `@ExtendWith(MockitoExtension.class)` with `@Mock` and `@InjectMocks` for unit tests; avoid `MockitoAnnotations.openMocks(this)` in JUnit 5
- Use AssertJ (`assertThat(...)`) over JUnit `assertEquals`; chain assertions for readability and better failure messages
- Use Testcontainers for database, Kafka, and Redis integration tests; define containers as `static` fields annotated with `@Container` and `@Testcontainers`
- Annotate slow integration tests with a custom `@Tag("integration")` and exclude them from the default Surefire run; run them separately in CI

## MUST NOT
- Do not use `@SpringBootTest` for unit tests — it loads the full context and slows feedback loops
- Do not mock types you do not own (e.g., `HttpServletRequest`) — use fakes or Spring's `MockHttpServletRequest`
- Do not rely on field order in JSON assertion strings; use `JSONAssert` or AssertJ `satisfies` blocks
- Do not use `Thread.sleep` for async assertions; use `Awaitility.await().atMost(...)` instead
- Do not share mutable static state between tests; reset it in `@AfterEach`

## Edge Case Checklist
- `@Transactional` on test methods rolls back by default — verify this is the intended behaviour for each test
- Quarkus `@QuarkusTest` starts a real HTTP server; use `@TestHTTPEndpoint` to avoid hardcoded ports
- Micronaut `@MicronautTest` reuses the application context — mark tests that mutate state with `@Singleton` scope isolation
- Kotlin data class `equals`/`hashCode`: assert structural equality, not reference equality
- Testcontainers startup time: use `@Shared` (Spock) or static containers to reuse containers across test methods
