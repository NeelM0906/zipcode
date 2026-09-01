from __future__ import annotations

import unittest

from retry_batch import RetryPolicy, run_batch


class RetryPolicyTests(unittest.TestCase):
    def test_exponential_delay_is_capped(self) -> None:
        policy = RetryPolicy(base_delay=0.5, max_delay=3.0)
        self.assertEqual([policy.delay_after(i) for i in range(1, 6)], [0.5, 1.0, 2.0, 3.0, 3.0])

    def test_invalid_policy_is_rejected(self) -> None:
        with self.assertRaises(ValueError):
            RetryPolicy(max_attempts=0)
        with self.assertRaises(ValueError):
            RetryPolicy(base_delay=-1)


class RunBatchTests(unittest.TestCase):
    def test_successes_are_keyed_by_input(self) -> None:
        result = run_batch([2, 3], lambda value: value * 10)
        self.assertEqual(result.successes, {2: 20, 3: 30})
        self.assertEqual(result.attempts, {2: 1, 3: 1})
        self.assertEqual(result.failures, {})

    def test_final_attempt_can_succeed(self) -> None:
        calls = 0

        def flaky(value: str) -> str:
            nonlocal calls
            calls += 1
            if calls < 3:
                raise RuntimeError("transient")
            return value.upper()

        result = run_batch(["ok"], flaky, policy=RetryPolicy(max_attempts=3))
        self.assertEqual(result.successes, {"ok": "OK"})
        self.assertEqual(result.attempts, {"ok": 3})
        self.assertEqual(result.failures, {})

    def test_exhausted_failure_is_preserved_without_final_sleep(self) -> None:
        sleeps: list[float] = []

        def fail(_: int) -> int:
            raise RuntimeError("still broken")

        result = run_batch(
            [7],
            fail,
            policy=RetryPolicy(max_attempts=3, base_delay=0.25),
            sleep=sleeps.append,
        )
        self.assertEqual(result.successes, {})
        self.assertIsInstance(result.failures[7], RuntimeError)
        self.assertEqual(result.attempts, {7: 3})
        self.assertEqual(sleeps, [0.25, 0.5])

    def test_non_retryable_exception_escapes_immediately(self) -> None:
        calls = 0

        def invalid(_: int) -> int:
            nonlocal calls
            calls += 1
            raise ValueError("invalid input")

        with self.assertRaises(ValueError):
            run_batch([1], invalid, retry_on=(RuntimeError,))
        self.assertEqual(calls, 1)

    def test_generator_input_is_supported(self) -> None:
        result = run_batch((value for value in range(3)), lambda value: value + 1)
        self.assertEqual(result.successes, {0: 1, 1: 2, 2: 3})


if __name__ == "__main__":
    unittest.main()
