"""Unit tests for the fuzz checks' rules (OT-011). Run: uv run --project apifuzz pytest."""

import pytest

from hooks import error_body_problem, leaked_internal_text

DENIED = [
    "Database error: connection refused",
    "Internal error: something broke",
    'error returned from database: duplicate key value violates unique constraint "users_email_key"',
    'relation "users" does not exist',
    "SQLSTATE 23505",
    "sqlx::Error::RowNotFound",
    "thread 'tokio-runtime-worker' panicked at src/main.rs:12:5",
    "at src/infrastructure/api/error.rs:300",
    "note: run with `RUST_BACKTRACE=1`",
    "new row violates check constraint",
]

ALLOWED = [
    "Invalid email or password",
    "Missing authentication token",
    "Failed to deserialize the JSON body into the target type: invalid type: integer `5`, expected a string at line 1 column 9",
    "Failed to parse the request body as JSON: EOF while parsing a value at line 1 column 0",
    "Device fingerprint is required",
    "Verification code has expired",
    "Too many attempts; try again later",
]


@pytest.mark.parametrize("message", DENIED)
def test_internal_text_is_denied(message: str) -> None:
    assert leaked_internal_text(message) is not None


@pytest.mark.parametrize("message", ALLOWED)
def test_client_facing_text_is_allowed(message: str) -> None:
    assert leaked_internal_text(message) is None


def test_echoed_client_input_is_not_a_leak() -> None:
    message = "Unknown provider: SQLSTATE"
    assert leaked_internal_text(message, "DELETE /api/v1/auth/oauth/SQLSTATE/unlink") is None
    assert leaked_internal_text(message, "DELETE /api/v1/auth/oauth/google/unlink") is not None


def test_standard_and_verification_shapes_pass() -> None:
    assert error_body_problem(400, "application/json", {"code": "validation_error", "message": "Bad"}) is None
    assert error_body_problem(400, "application/json", {"error": "Wrong code", "error_code": "invalid_code"}) is None
    assert error_body_problem(204, "", None) is None


@pytest.mark.parametrize(
    ("status", "content_type", "body", "expected"),
    [
        (415, "application/json", {"code": "x", "message": "y"}, "not 415"),
        (422, "text/plain", None, "not 422"),
        (405, "", None, "not JSON"),
        (400, "text/plain; charset=utf-8", None, "not JSON"),
        (400, "application/json", ["not", "an", "object"], "not a JSON object"),
        (401, "application/json", {"message": "no code"}, "neither"),
        (500, "application/json", {"code": "internal_error", "message": "Internal error: boom"}, "internal text"),
    ],
)
def test_contract_violations_are_reported(status: int, content_type: str, body: object, expected: str) -> None:
    problem = error_body_problem(status, content_type, body)
    assert problem is not None and expected in problem
