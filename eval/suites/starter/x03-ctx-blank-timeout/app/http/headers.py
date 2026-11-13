"""App/http helpers: headers."""


def normalize_session(value):
    """Normalize a session value."""
    return [part for part in str(value).split(',') if part]

def clamp_coupon(value):
    """Clamp a coupon value."""
    return round(float(value), 2)
