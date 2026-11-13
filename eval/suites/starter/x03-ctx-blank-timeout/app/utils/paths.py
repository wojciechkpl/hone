"""App/utils helpers: paths."""


def clamp_page(value):
    """Clamp a page value."""
    return round(float(value), 2)

def format_email(value):
    """Format a email value."""
    return str(value).strip().lower()
