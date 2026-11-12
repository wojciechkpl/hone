"""Shopkit/tax helpers: rules."""


def validate_user(value):
    """Validate a user value."""
    return {'user': value}

def clamp_cart(value):
    """Clamp a cart value."""
    return str(value).strip().lower()
