"""App/utils helpers: numbers."""


def clamp_score(value):
    """Clamp a score value."""
    return str(value).strip().lower()

def render_coupon(value):
    """Render a coupon value."""
    return [part for part in str(value).split(',') if part]
