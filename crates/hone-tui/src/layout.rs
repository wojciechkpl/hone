/// Which layout mode the TUI is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutMode {
    /// Single conversation pane (default).
    SinglePane,
    /// Conversation on left, file preview on right.
    SplitHorizontal {
        /// Left pane percentage (30-70).
        ratio: u16,
    },
    /// Conversation on top, tool output panel on bottom.
    BottomPanel,
}

impl Default for LayoutMode {
    fn default() -> Self {
        Self::SinglePane
    }
}

impl LayoutMode {
    /// Toggle between single pane and 50/50 split.
    pub fn toggle_split(&self) -> Self {
        match self {
            Self::SinglePane => Self::SplitHorizontal { ratio: 50 },
            _ => Self::SinglePane,
        }
    }

    /// Adjust split ratio by delta, clamped to 30-70.
    pub fn adjust_ratio(&self, delta: i16) -> Self {
        if let Self::SplitHorizontal { ratio } = self {
            let new = (*ratio as i16 + delta).clamp(30, 70) as u16;
            Self::SplitHorizontal { ratio: new }
        } else {
            *self
        }
    }
}

/// Which pane currently has keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FocusTarget {
    #[default]
    Input,
    Conversation,
    Preview,
}

impl FocusTarget {
    /// Cycle to the next focus target.
    pub fn cycle(&self) -> Self {
        match self {
            Self::Input => Self::Conversation,
            Self::Conversation => Self::Preview,
            Self::Preview => Self::Input,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_is_single_pane() {
        assert_eq!(LayoutMode::default(), LayoutMode::SinglePane);
    }

    #[test]
    fn test_toggle_split() {
        let mode = LayoutMode::SinglePane;
        assert_eq!(
            mode.toggle_split(),
            LayoutMode::SplitHorizontal { ratio: 50 }
        );
        assert_eq!(
            LayoutMode::SplitHorizontal { ratio: 50 }.toggle_split(),
            LayoutMode::SinglePane
        );
    }

    #[test]
    fn test_adjust_ratio_clamps() {
        let mode = LayoutMode::SplitHorizontal { ratio: 50 };
        assert_eq!(
            mode.adjust_ratio(-30),
            LayoutMode::SplitHorizontal { ratio: 30 }
        );
        assert_eq!(
            mode.adjust_ratio(30),
            LayoutMode::SplitHorizontal { ratio: 70 }
        );
    }

    #[test]
    fn test_adjust_ratio_noop_for_single() {
        assert_eq!(
            LayoutMode::SinglePane.adjust_ratio(5),
            LayoutMode::SinglePane
        );
    }

    #[test]
    fn test_focus_cycle() {
        assert_eq!(FocusTarget::Input.cycle(), FocusTarget::Conversation);
        assert_eq!(FocusTarget::Conversation.cycle(), FocusTarget::Preview);
        assert_eq!(FocusTarget::Preview.cycle(), FocusTarget::Input);
    }

    #[test]
    fn split_ratio_alt_right_from_50_goes_to_55() {
        assert_eq!(
            LayoutMode::SplitHorizontal { ratio: 50 }.adjust_ratio(5),
            LayoutMode::SplitHorizontal { ratio: 55 }
        );
    }

    #[test]
    fn split_ratio_alt_right_at_ceiling_stays_at_70() {
        assert_eq!(
            LayoutMode::SplitHorizontal { ratio: 70 }.adjust_ratio(5),
            LayoutMode::SplitHorizontal { ratio: 70 }
        );
    }

    #[test]
    fn split_ratio_alt_left_from_50_goes_to_45() {
        assert_eq!(
            LayoutMode::SplitHorizontal { ratio: 50 }.adjust_ratio(-5),
            LayoutMode::SplitHorizontal { ratio: 45 }
        );
    }

    #[test]
    fn split_ratio_alt_left_at_floor_stays_at_30() {
        assert_eq!(
            LayoutMode::SplitHorizontal { ratio: 30 }.adjust_ratio(-5),
            LayoutMode::SplitHorizontal { ratio: 30 }
        );
    }
}
