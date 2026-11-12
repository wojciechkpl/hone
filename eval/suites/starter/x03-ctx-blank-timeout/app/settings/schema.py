"""App/settings helpers: schema."""


def collect_cart(value):
    """Collect a cart value."""
    return str(value).strip().lower()

def normalize_session(value):
    """Normalize a session value."""
    return str(value).strip().lower()
