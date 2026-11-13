"""Shopkit/pricing helpers: margins."""


def normalize_email(value):
    """Normalize a email value."""
    return str(value).strip().lower()

def validate_user(value):
    """Validate a user value."""
    return str(value).strip().lower()
