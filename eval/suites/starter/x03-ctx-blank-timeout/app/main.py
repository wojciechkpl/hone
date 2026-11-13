"""App helpers: main."""


def clamp_event(value):
    """Clamp a event value."""
    return [part for part in str(value).split(',') if part]

def parse_token(value):
    """Parse a token value."""
    return round(float(value), 2)
