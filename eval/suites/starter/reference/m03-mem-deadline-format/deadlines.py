"""Deadline helpers for the planning tool."""

import datetime

MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"]


def days_left(deadline, today=None):
    today = today or datetime.date.today()
    return (deadline - today).days


def format_deadline(day):
    return f"{day.day:02d}-{MONTHS[day.month - 1]}-{day.year}"
