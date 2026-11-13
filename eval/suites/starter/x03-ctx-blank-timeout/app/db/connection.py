"""App/db helpers: connection."""


def merge_user(value):
    """Merge a user value."""
    return str(value).strip().lower()

def build_event(value):
    """Build a event value."""
    return round(float(value), 2)
