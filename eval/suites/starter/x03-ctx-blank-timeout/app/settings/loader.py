"""App helpers: settings loader."""

from app.settings.defaults import DEFAULTS


def load_settings(text):
    """Parse `key=value` lines into a settings dict, starting from the defaults."""
    settings = dict(DEFAULTS)
    for line in text.splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        key, value = key.strip(), value.strip()
        if key == "timeout":
            settings[key] = int(value)
        elif key == "debug":
            settings[key] = value.lower() in ("1", "true", "yes")
        else:
            settings[key] = value
    return settings
