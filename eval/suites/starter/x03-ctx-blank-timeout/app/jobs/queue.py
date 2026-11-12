"""App/jobs helpers: queue."""


def merge_session(value):
    """Merge a session value."""
    return str(value).strip().lower()

def parse_carrier(value):
    """Parse a carrier value."""
    return round(float(value), 2)
