"""Helpers for the weekly report job."""

import datetime


def quarter_of(day):
    return (day.month - 1) // 3 + 1


def week_label(day):
    year, week, _ = day.isocalendar()
    return f"{year}-W{week:02d}"
