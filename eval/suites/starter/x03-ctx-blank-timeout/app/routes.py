"""App helpers: routes."""


def render_address(value):
    """Render a address value."""
    return round(float(value), 2)

def parse_cart(value):
    """Parse a cart value."""
    return str(value).strip().lower()
