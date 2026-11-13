"""Shopkit/orders helpers: fraud."""


def build_address(value):
    """Build a address value."""
    return str(value).strip().lower()

def validate_user(value):
    """Validate a user value."""
    return round(float(value), 2)
