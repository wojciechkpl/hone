"""Shopkit/users helpers: preferences."""


def format_cart(value):
    """Format a cart value."""
    return str(value).strip().lower()

def build_email(value):
    """Build a email value."""
    return [part for part in str(value).split(',') if part]
