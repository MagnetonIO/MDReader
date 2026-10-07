use crate::{ReaderError, Result};

#[derive(Default)]
pub(crate) struct Query {
    pub terms: Vec<String>,
    pub tags: Vec<String>,
    pub paths: Vec<String>,
    pub linked: Vec<String>,
    pub after: Vec<i64>,
}
impl Query {
    pub fn parse(input: &str) -> Result<Self> {
        let fail = || ReaderError::Invalid("invalid search filter or quoted phrase".into());
        if input.trim().is_empty() || input.len() > 1024 {
            return Err(fail());
        }
        let mut tokens = Vec::new();
        let mut token = String::new();
        let mut quoted = false;
        let mut literal = false;
        for ch in input.chars() {
            if ch == '"' {
                if token.is_empty() && !quoted {
                    literal = true;
                }
                quoted = !quoted;
            } else if ch.is_whitespace() && !quoted {
                if !token.is_empty() {
                    tokens.push((std::mem::take(&mut token), literal));
                    literal = false;
                }
            } else {
                token.push(ch);
            }
        }
        if quoted {
            return Err(fail());
        }
        if !token.is_empty() {
            tokens.push((token, literal));
        }
        let mut query = Self::default();
        for (token, literal) in tokens {
            if literal {
                query.terms.push(token);
                continue;
            }
            if let Some((key, value)) = token.split_once(':') {
                if value.is_empty() {
                    return Err(fail());
                }
                match key {
                    "tag" => query
                        .tags
                        .push(value.trim_start_matches('#').to_lowercase()),
                    "path" => query.paths.push(value.into()),
                    "linked-to" => query.linked.push(value.into()),
                    "after" => {
                        if !value.is_ascii()
                            || value.len() != 10
                            || &value[4..5] != "-"
                            || &value[7..8] != "-"
                        {
                            return Err(fail());
                        }
                        let year: i32 = value[..4].parse().map_err(|_| fail())?;
                        let month: u8 = value[5..7].parse().map_err(|_| fail())?;
                        let day: u8 = value[8..].parse().map_err(|_| fail())?;
                        let date = time::Date::from_calendar_date(
                            year,
                            time::Month::try_from(month).map_err(|_| fail())?,
                            day,
                        )
                        .map_err(|_| fail())?;
                        query
                            .after
                            .push(i64::from(date.to_julian_day() - 2_440_588) * 86_400_000);
                    }
                    _ => return Err(fail()),
                }
            } else {
                query.terms.push(token);
            }
        }
        if query.terms.is_empty()
            && query.tags.is_empty()
            && query.paths.is_empty()
            && query.linked.is_empty()
            && query.after.is_empty()
        {
            return Err(fail());
        }
        Ok(query)
    }
    pub fn fts(&self) -> String {
        self.terms
            .iter()
            .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" AND ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quoted_colons_are_literal_and_quoted_filter_values_stay_filters() {
        let query = Query::parse("path:\"notes with spaces\" \"literal:value\"").unwrap();
        assert_eq!(query.paths, vec!["notes with spaces"]);
        assert_eq!(query.terms, vec!["literal:value"]);
        assert!(Query::parse("unknown:value").is_err());
    }
}
