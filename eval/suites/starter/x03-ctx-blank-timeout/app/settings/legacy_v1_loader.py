"""Legacy v1 settings loader, kept for the old migration script (not used by the app).

In v1 an empty `timeout=` entry in the settings file was an error on purpose.
"""


def load_legacy_settings(text):
    settings = {}
    for line in text.splitlines():
        if "=" in line:
            key, value = line.split("=", 1)
            settings[key.strip()] = value.strip()
    if "timeout" in settings:
        if not settings["timeout"]:
            raise ValueError("timeout must not be empty")
        settings["timeout"] = int(settings["timeout"])
    return settings
