/**
 * NodeSpace Design System Tokens
 *
 * Comprehensive design token system following professional blue accent theme
 * with campaign-aligned aesthetics for light/dark mode support.
 */

// Color tokens following campaign specifications
export interface ColorTokens {
  // Brand colors
  primary: {
    50: string;
    100: string;
    200: string;
    300: string;
    400: string;
    500: string; // Main brand color
    600: string;
    700: string;
    800: string;
    900: string;
  };

  // Surface colors for backgrounds and containers
  surface: {
    background: string;
    elevated: string;
    overlay: string;
    panel: string;
    input: string;
  };

  // Text colors
  text: {
    primary: string;
    secondary: string;
    tertiary: string;
    inverse: string;
    disabled: string;
    placeholder: string;
  };

  // Border colors
  border: {
    subtle: string;
    default: string;
    strong: string;
    accent: string;
  };

  // State colors
  state: {
    success: string;
    warning: string;
    error: string;
    info: string;
  };

  // Interactive element colors
  interactive: {
    idle: string;
    hover: string;
    active: string;
    disabled: string;
  };
}

// Typography scale and font families
export interface TypographyTokens {
  fontFamily: {
    ui: string;
    mono: string;
    brand: string;
  };

  fontSize: {
    xs: string;
    sm: string;
    base: string;
    lg: string;
    xl: string;
    '2xl': string;
    '3xl': string;
    '4xl': string;
    '5xl': string;
  };

  fontWeight: {
    normal: number;
    medium: number;
    semibold: number;
    bold: number;
  };

  lineHeight: {
    tight: number;
    normal: number;
    relaxed: number;
  };

  letterSpacing: {
    tight: string;
    normal: string;
    wide: string;
  };
}

// Spacing scale using consistent 4px base
export interface SpacingTokens {
  0: string;
  1: string; // 4px
  2: string; // 8px
  3: string; // 12px
  4: string; // 16px
  5: string; // 20px
  6: string; // 24px
  8: string; // 32px
  10: string; // 40px
  12: string; // 48px
  16: string; // 64px
  20: string; // 80px
  24: string; // 96px
  32: string; // 128px
  40: string; // 160px
  48: string; // 192px
  56: string; // 224px
  64: string; // 256px
}

// Shadow system for elevation and depth
export interface ShadowTokens {
  none: string;
  xs: string; // Subtle shadow
  sm: string; // Small shadow
  base: string; // Default shadow
  md: string; // Medium shadow
  lg: string; // Large shadow
  xl: string; // Extra large shadow
  '2xl': string; // Maximum depth
  inner: string; // Inner shadow for inset effects
}

// Border radius values
export interface RadiusTokens {
  none: string;
  sm: string;
  base: string;
  md: string;
  lg: string;
  xl: string;
  '2xl': string;
  '3xl': string;
  full: string;
}

// Animation and transition tokens
export interface AnimationTokens {
  duration: {
    instant: string;
    fast: string;
    normal: string;
    slow: string;
  };

  easing: {
    linear: string;
    ease: string;
    easeIn: string;
    easeOut: string;
    easeInOut: string;
  };
}

// Responsive breakpoints
export interface BreakpointTokens {
  sm: string; // 640px
  md: string; // 768px
  lg: string; // 1024px
  xl: string; // 1280px
  '2xl': string; // 1536px
}

// Complete design tokens interface
export interface DesignTokens {
  color: ColorTokens;
  typography: TypographyTokens;
  spacing: SpacingTokens;
  shadow: ShadowTokens;
  radius: RadiusTokens;
  animation: AnimationTokens;
  breakpoint: BreakpointTokens;
}

// Light theme tokens (Campaign reference)
export const lightTokens: DesignTokens = {
  color: {
    primary: {
      50: '#eff6ff',
      100: '#dbeafe',
      200: '#bfdbfe',
      300: '#93c5fd',
      400: '#60a5fa',
      500: '#007acc', // Professional blue from campaign
      600: '#0066b3',
      700: '#005299',
      800: '#003d80',
      900: '#002966'
    },

    surface: {
      background: '#ffffff', // Pure white background
      elevated: '#ffffff', // Elevated surfaces (modals, dropdowns)
      overlay: 'rgba(0, 0, 0, 0.1)', // Overlay backgrounds
      panel: '#f8f9fa', // Panel backgrounds
      input: '#ffffff' // Input field backgrounds
    },

    text: {
      primary: '#1a1a1a', // Primary text - dark charcoal
      secondary: '#333333', // Secondary text
      tertiary: '#666666', // Tertiary text - medium gray
      inverse: '#ffffff', // Text on dark backgrounds
      disabled: '#999999', // Disabled text
      placeholder: '#9ca3af' // Placeholder text
    },

    border: {
      subtle: '#f3f4f6', // Very subtle borders
      default: '#e1e5e9', // Default borders
      strong: '#d1d5db', // Strong borders
      accent: '#007acc' // Accent borders
    },

    state: {
      success: '#10b981',
      warning: '#f59e0b',
      error: '#ef4444',
      info: '#007acc'
    },

    interactive: {
      idle: '#007acc',
      hover: '#0066b3',
      active: '#005299',
      disabled: '#9ca3af'
    }
  },

  typography: {
    fontFamily: {
      ui: '"SF Mono", Monaco, "Cascadia Code", "Roboto Mono", Consolas, monospace',
      mono: '"SF Mono", Monaco, "Cascadia Code", "Roboto Mono", Consolas, monospace',
      brand: '"SF Mono", Monaco, "Cascadia Code", "Roboto Mono", Consolas, monospace'
    },

    fontSize: {
      xs: '0.75rem', // 12px
      sm: '0.875rem', // 14px
      base: '1rem', // 16px
      lg: '1.125rem', // 18px
      xl: '1.25rem', // 20px
      '2xl': '1.5rem', // 24px
      '3xl': '1.875rem', // 30px
      '4xl': '2.25rem', // 36px
      '5xl': '3rem' // 48px
    },

    fontWeight: {
      normal: 400,
      medium: 500,
      semibold: 600,
      bold: 700
    },

    lineHeight: {
      tight: 1.25,
      normal: 1.5,
      relaxed: 1.75
    },

    letterSpacing: {
      tight: '-0.025em',
      normal: '0em',
      wide: '0.025em'
    }
  },

  spacing: {
    0: '0',
    1: '0.25rem', // 4px
    2: '0.5rem', // 8px
    3: '0.75rem', // 12px
    4: '1rem', // 16px
    5: '1.25rem', // 20px
    6: '1.5rem', // 24px
    8: '2rem', // 32px
    10: '2.5rem', // 40px
    12: '3rem', // 48px
    16: '4rem', // 64px
    20: '5rem', // 80px
    24: '6rem', // 96px
    32: '8rem', // 128px
    40: '10rem', // 160px
    48: '12rem', // 192px
    56: '14rem', // 224px
    64: '16rem' // 256px
  },

  shadow: {
    none: 'none',
    xs: '0 1px 2px 0 rgba(0, 0, 0, 0.05)',
    sm: '0 1px 3px 0 rgba(0, 0, 0, 0.1), 0 1px 2px -1px rgba(0, 0, 0, 0.1)',
    base: '0 1px 3px 0 rgba(0, 0, 0, 0.1), 0 1px 2px -1px rgba(0, 0, 0, 0.1)',
    md: '0 4px 6px -1px rgba(0, 0, 0, 0.1), 0 2px 4px -2px rgba(0, 0, 0, 0.1)',
    lg: '0 10px 15px -3px rgba(0, 0, 0, 0.1), 0 4px 6px -4px rgba(0, 0, 0, 0.1)',
    xl: '0 20px 25px -5px rgba(0, 0, 0, 0.1), 0 8px 10px -6px rgba(0, 0, 0, 0.1)',
    '2xl': '0 25px 50px -12px rgba(0, 0, 0, 0.25)',
    inner: 'inset 0 2px 4px 0 rgba(0, 0, 0, 0.05)'
  },

  radius: {
    none: '0',
    sm: '0.125rem', // 2px
    base: '0.25rem', // 4px
    md: '0.375rem', // 6px
    lg: '0.5rem', // 8px
    xl: '0.75rem', // 12px
    '2xl': '1rem', // 16px
    '3xl': '1.5rem', // 24px
    full: '9999px'
  },

  animation: {
    duration: {
      instant: '0ms',
      fast: '150ms',
      normal: '300ms',
      slow: '500ms'
    },

    easing: {
      linear: 'linear',
      ease: 'ease',
      easeIn: 'cubic-bezier(0.4, 0, 1, 1)',
      easeOut: 'cubic-bezier(0, 0, 0.2, 1)',
      easeInOut: 'cubic-bezier(0.4, 0, 0.2, 1)'
    }
  },

  breakpoint: {
    sm: '640px',
    md: '768px',
    lg: '1024px',
    xl: '1280px',
    '2xl': '1536px'
  }
};

// Dark theme tokens (Campaign reference)
export const darkTokens: DesignTokens = {
  ...lightTokens, // Inherit non-color tokens

  color: {
    primary: {
      50: '#002966',
      100: '#003d80',
      200: '#005299',
      300: '#0066b3',
      400: '#007acc',
      500: '#0099ff', // Brighter blue for dark theme
      600: '#33aaff',
      700: '#66bbff',
      800: '#99ccff',
      900: '#cce6ff'
    },

    surface: {
      background: '#1a1a1a', // Dark navy/charcoal background
      elevated: '#2a2a2a', // Elevated surfaces
      overlay: 'rgba(255, 255, 255, 0.1)', // Light overlay on dark
      panel: '#2a2a2a', // Panel backgrounds
      input: '#374151' // Input field backgrounds
    },

    text: {
      primary: '#ffffff', // White primary text
      secondary: '#e0e0e0', // Light gray secondary text
      tertiary: '#888888', // Medium gray tertiary text
      inverse: '#1a1a1a', // Dark text on light backgrounds
      disabled: '#666666', // Disabled text
      placeholder: '#9ca3af' // Placeholder text
    },

    border: {
      subtle: '#374151', // Subtle borders in dark theme
      default: '#4b5563', // Default borders
      strong: '#6b7280', // Strong borders
      accent: '#0099ff' // Accent borders
    },

    state: {
      success: '#10b981',
      warning: '#f59e0b',
      error: '#ef4444',
      info: '#0099ff'
    },

    interactive: {
      idle: '#0099ff',
      hover: '#33aaff',
      active: '#66bbff',
      disabled: '#666666'
    }
  }
};

// Theme type for runtime theme switching
export type Theme = 'light' | 'dark' | 'system';

// Get tokens for specified theme
export function getTokens(theme: Theme, systemTheme?: 'light' | 'dark'): DesignTokens {
  if (theme === 'system') {
    const resolvedTheme =
      systemTheme ||
      (typeof window !== 'undefined' && window.matchMedia('(prefers-color-scheme: dark)').matches
        ? 'dark'
        : 'light');
    return resolvedTheme === 'dark' ? darkTokens : lightTokens;
  }

  return theme === 'dark' ? darkTokens : lightTokens;
}

// Node-specific visual tokens
export interface NodeTokens {
  // Node type colors
  nodeType: {
    text: {
      background: string;
      border: string;
      accent: string;
    };
    task: {
      background: string;
      border: string;
      accent: string;
    };
    aiChat: {
      background: string;
      border: string;
      accent: string;
    };
    entity: {
      background: string;
      border: string;
      accent: string;
    };
    query: {
      background: string;
      border: string;
      accent: string;
    };
  };

  // Node interaction states
  state: {
    idle: {
      background: string;
      border: string;
      shadow: string;
    };
    hover: {
      background: string;
      border: string;
      shadow: string;
    };
    focus: {
      background: string;
      border: string;
      shadow: string;
    };
    active: {
      background: string;
      border: string;
      shadow: string;
    };
    selected: {
      background: string;
      border: string;
      shadow: string;
    };
    disabled: {
      background: string;
      border: string;
      opacity: number;
    };
  };
}

// Node tokens for light theme
export const lightNodeTokens: NodeTokens = {
  nodeType: {
    text: {
      background: lightTokens.color.surface.background,
      border: lightTokens.color.border.default,
      accent: '#10b981' // Green for text nodes
    },
    task: {
      background: lightTokens.color.surface.background,
      border: lightTokens.color.border.default,
      accent: '#f59e0b' // Orange for task nodes
    },
    aiChat: {
      background: lightTokens.color.surface.background,
      border: lightTokens.color.border.default,
      accent: lightTokens.color.primary[500] // Blue for AI chat
    },
    entity: {
      background: lightTokens.color.surface.background,
      border: lightTokens.color.border.default,
      accent: '#8b5cf6' // Purple for entities
    },
    query: {
      background: lightTokens.color.surface.background,
      border: lightTokens.color.border.default,
      accent: '#ec4899' // Pink for queries
    }
  },

  state: {
    idle: {
      background: lightTokens.color.surface.background,
      border: lightTokens.color.border.default,
      shadow: lightTokens.shadow.sm
    },
    hover: {
      background: lightTokens.color.surface.panel,
      border: lightTokens.color.border.strong,
      shadow: lightTokens.shadow.md
    },
    focus: {
      background: lightTokens.color.surface.background,
      border: lightTokens.color.primary[500],
      shadow: `0 0 0 3px ${lightTokens.color.primary[200]}`
    },
    active: {
      background: lightTokens.color.surface.panel,
      border: lightTokens.color.primary[600],
      shadow: lightTokens.shadow.inner
    },
    selected: {
      background: lightTokens.color.primary[50],
      border: lightTokens.color.primary[500],
      shadow: lightTokens.shadow.md
    },
    disabled: {
      background: '#f9fafb',
      border: '#e5e7eb',
      opacity: 0.6
    }
  }
};

// Node tokens for dark theme
export const darkNodeTokens: NodeTokens = {
  nodeType: {
    text: {
      background: darkTokens.color.surface.background,
      border: darkTokens.color.border.default,
      accent: '#10b981'
    },
    task: {
      background: darkTokens.color.surface.background,
      border: darkTokens.color.border.default,
      accent: '#f59e0b'
    },
    aiChat: {
      background: darkTokens.color.surface.background,
      border: darkTokens.color.border.default,
      accent: darkTokens.color.primary[500]
    },
    entity: {
      background: darkTokens.color.surface.background,
      border: darkTokens.color.border.default,
      accent: '#8b5cf6'
    },
    query: {
      background: darkTokens.color.surface.background,
      border: darkTokens.color.border.default,
      accent: '#ec4899'
    }
  },

  state: {
    idle: {
      background: darkTokens.color.surface.background,
      border: darkTokens.color.border.default,
      shadow: 'none'
    },
    hover: {
      background: darkTokens.color.surface.elevated,
      border: darkTokens.color.border.strong,
      shadow: darkTokens.shadow.md
    },
    focus: {
      background: darkTokens.color.surface.background,
      border: darkTokens.color.primary[500],
      shadow: `0 0 0 3px ${darkTokens.color.primary[800]}`
    },
    active: {
      background: darkTokens.color.surface.elevated,
      border: darkTokens.color.primary[400],
      shadow: 'inset 0 2px 4px 0 rgba(0, 0, 0, 0.3)'
    },
    selected: {
      background: darkTokens.color.primary[900],
      border: darkTokens.color.primary[500],
      shadow: darkTokens.shadow.md
    },
    disabled: {
      background: '#374151',
      border: '#4b5563',
      opacity: 0.6
    }
  }
};

// Get node tokens for specified theme
export function getNodeTokens(theme: Theme, systemTheme?: 'light' | 'dark'): NodeTokens {
  if (theme === 'system') {
    const resolvedTheme =
      systemTheme ||
      (typeof window !== 'undefined' && window.matchMedia('(prefers-color-scheme: dark)').matches
        ? 'dark'
        : 'light');
    return resolvedTheme === 'dark' ? darkNodeTokens : lightNodeTokens;
  }

  return theme === 'dark' ? darkNodeTokens : lightNodeTokens;
}
