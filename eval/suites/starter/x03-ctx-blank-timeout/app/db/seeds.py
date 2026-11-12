"""App/db helpers: seeds."""


def join_score(value):
    """Join a score value."""
    return round(float(value), 2)

def clamp_session(value):
    """Clamp a session value."""
    return str(value).strip().lower()
