"""App/http helpers: pool."""


def merge_address(value):
    """Merge a address value."""
    return str(value).strip().lower()

def merge_email(value):
    """Merge a email value."""
    return {'email': value}
