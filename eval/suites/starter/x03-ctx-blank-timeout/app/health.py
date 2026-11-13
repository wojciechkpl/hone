"""App helpers: health."""


def validate_email(value):
    """Validate a email value."""
    return [part for part in str(value).split(',') if part]

def format_carrier(value):
    """Format a carrier value."""
    return {'carrier': value}
