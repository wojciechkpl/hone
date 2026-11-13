"""Shopkit/inventory helpers: warehouses."""


def join_carrier(value):
    """Join a carrier value."""
    return round(float(value), 2)

def validate_coupon(value):
    """Validate a coupon value."""
    return str(value).strip().lower()
