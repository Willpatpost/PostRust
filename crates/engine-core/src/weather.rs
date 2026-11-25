//! Weather system for Era-2 package C (particles + weather).
//!
//! The weather model provides:
//! - Weather state (Clear, Rain)
//! - Rain intensity in [0,1]
//! - Composition with day/night cycle

/// The weather state
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WeatherClass {
    /// Clear weather
    Clear,
    /// Rainy weather
    Rain,
}

impl WeatherClass {
    /// Returns true if it's raining
    pub fn is_raining(self) -> bool {
        matches!(self, WeatherClass::Rain)
    }
}

/// The weather state with intensity
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Weather {
    /// The weather class (Clear or Rain)
    pub class: WeatherClass,
    /// Rain intensity in [0,1]
    pub intensity: f32,
}

impl Weather {
    /// Create a clear weather state
    pub fn clear() -> Self {
        Self {
            class: WeatherClass::Clear,
            intensity: 0.0,
        }
    }

    /// Create a rainy weather state with the given intensity
    pub fn rain(intensity: f32) -> Self {
        Self {
            class: WeatherClass::Rain,
            intensity: intensity.clamp(0.0, 1.0),
        }
    }

    /// Returns true if it's raining
    pub fn is_raining(self) -> bool {
        self.class.is_raining()
    }

    /// Get the effective rain intensity
    pub fn rain_intensity(self) -> f32 {
        if self.is_raining() {
            self.intensity
        } else {
            0.0
        }
    }
}

impl Default for Weather {
    fn default() -> Self {
        Self::clear()
    }
}

/// Weather configuration for a frame
pub struct WeatherFrame {
    /// The weather state
    pub weather: Weather,
    /// Day/night factor in [0,1]
    pub day_night: f32,
}

impl WeatherFrame {
    /// Create a new weather frame
    pub fn new(weather: Weather, day_night: f32) -> Self {
        Self {
            weather,
            day_night: day_night.clamp(0.0, 1.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_weather_default() {
        let weather = Weather::default();
        assert_eq!(weather.class, WeatherClass::Clear);
        assert_eq!(weather.intensity, 0.0);
    }

    #[test]
    fn test_weather_rain() {
        let weather = Weather::rain(0.5);
        assert_eq!(weather.class, WeatherClass::Rain);
        assert_eq!(weather.intensity, 0.5);
        assert!(weather.is_raining());
        assert_eq!(weather.rain_intensity(), 0.5);
    }

    #[test]
    fn test_weather_intensity_clamp() {
        let weather_low = Weather::rain(-0.5);
        let weather_high = Weather::rain(1.5);

        assert_eq!(weather_low.intensity, 0.0);
        assert_eq!(weather_high.intensity, 1.0);
    }

    #[test]
    fn test_weather_class() {
        assert!(!WeatherClass::Clear.is_raining());
        assert!(WeatherClass::Rain.is_raining());
    }
}
