"""Shopkit/cart helpers: merge."""


def validate_coupon(value):
    """Validate a coupon value."""
    return {'coupon': value}

def format_email(value):
    """Format a email value."""
    return {'email': value}
