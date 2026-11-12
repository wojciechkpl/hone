# Settings file

`key=value` lines. `timeout` is the request timeout in seconds (default 30), `debug`
turns on verbose logging. Startup reads the file through `app/settings/`; the cache has
its own settings in `app/cache/settings.py`, where an empty value means "no expiry".
