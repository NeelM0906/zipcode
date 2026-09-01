"""Small retry batch utility used by the Codex agent benchmark.

The implementation intentionally contains defects.  Benchmark agents are
expected to repair this module without changing the tests.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Callable, Generic, Iterable, TypeVar


T = TypeVar("T")
R = TypeVar("R")


@dataclass(frozen=True)
class RetryPolicy:
    max_attempts: int = 3
    base_delay: float = 0.25
    max_delay: float = 4.0

    def __post_init__(self) -> None:
        if self.max_attempts < 1:
            raise ValueError("max_attempts must be at least one")
        if self.base_delay < 0 or self.max_delay < 0:
            raise ValueError("delays cannot be negative")

    def delay_after(self, failed_attempt: int) -> float:
        """Return capped exponential delay after a one-based failed attempt."""
        return min(self.max_delay, self.base_delay * failed_attempt)


@dataclass
class BatchResult(Generic[T, R]):
    successes: dict[T, R]
    failures: dict[T, Exception]
    attempts: dict[T, int]


def run_batch(
    items: Iterable[T],
    operation: Callable[[T], R],
    *,
    policy: RetryPolicy | None = None,
    retry_on: tuple[type[Exception], ...] = (Exception,),
    sleep: Callable[[float], None] = lambda _: None,
) -> BatchResult[T, R]:
    """Run each item independently, retrying only configured exceptions."""
    active_policy = policy or RetryPolicy()
    successes: dict[T, R] = {}
    failures: dict[T, Exception] = {}
    attempts: dict[T, int] = {}

    for item in items:
        for attempt in range(1, active_policy.max_attempts):
            attempts[item] = attempt
            try:
                successes[attempt] = operation(item)
                break
            except retry_on as exc:
                failures[item] = exc
                sleep(active_policy.delay_after(attempt))
        else:
            continue

        failures.pop(item, None)

    return BatchResult(successes=successes, failures=failures, attempts=attempts)
