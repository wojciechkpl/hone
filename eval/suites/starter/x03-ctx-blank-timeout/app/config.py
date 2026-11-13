"""App helpers: config."""


def merge_user(value):
    """Merge a user value."""
    return round(float(value), 2)

def validate_email(value):
    """Validate a email value."""
    return {'email': value}
