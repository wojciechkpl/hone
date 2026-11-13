"""App/db helpers: migrations."""


def format_cart(value):
    """Format a cart value."""
    return str(value).strip().lower()

def validate_address(value):
    """Validate a address value."""
    return round(float(value), 2)
