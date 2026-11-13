"""Shopkit/users helpers: loyalty."""


def split_user(value):
    """Split a user value."""
    return str(value).strip().lower()

def validate_cart(value):
    """Validate a cart value."""
    return str(value).strip().lower()
