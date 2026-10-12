//! A small robots.txt reader. The longest matching Allow or Disallow wins.
//! An Allow of the same length as a Disallow wins.

#[derive(Clone, Debug)]
struct Rule {
    allow: bool,
    path: String,
}

#[derive(Clone, Debug, Default)]
pub struct Robots {
    gnosis: Vec<Rule>,
    star: Vec<Rule>,
}

impl Robots {
    pub fn parse(body: &str) -> Self {
        let mut gnosis = Vec::new();
        let mut star = Vec::new();
        let mut current: Vec<&str> = Vec::new();
        let flush = |agents: &mut Vec<&str>,
                     rules: &[Rule],
                     gnosis: &mut Vec<Rule>,
                     star: &mut Vec<Rule>| {
            if rules.is_empty() {
                agents.clear();
                return;
            }
            let gnosis_agent = agents
                .iter()
                .any(|a| a.eq_ignore_ascii_case("gnosis") || *a == "*");
            let only_star = agents.iter().all(|a| *a == "*");
            if agents.iter().any(|a| a.eq_ignore_ascii_case("gnosis")) {
                gnosis.extend(rules.iter().cloned());
            } else if only_star || gnosis_agent && agents.contains(&"*") {
                star.extend(rules.iter().cloned());
            }
            agents.clear();
        };
        let mut rules = Vec::new();
        for line in body.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let key = key.trim();
            let value = value.trim();
            if key.eq_ignore_ascii_case("user-agent") {
                if !rules.is_empty() {
                    flush(&mut current, &rules, &mut gnosis, &mut star);
                    rules.clear();
                }
                current.push(value);
            } else if key.eq_ignore_ascii_case("allow") {
                rules.push(Rule {
                    allow: true,
                    path: value.to_string(),
                });
            } else if key.eq_ignore_ascii_case("disallow") {
                rules.push(Rule {
                    allow: false,
                    path: value.to_string(),
                });
            }
        }
        flush(&mut current, &rules, &mut gnosis, &mut star);
        Self { gnosis, star }
    }

    pub fn allows(&self, path: &str) -> bool {
        let rules = if self.gnosis.is_empty() {
            &self.star
        } else {
            &self.gnosis
        };
        let mut best: Option<&Rule> = None;
        for rule in rules {
            if rule.path.is_empty() {
                continue;
            }
            if path.starts_with(&rule.path) {
                let replace = match best {
                    None => true,
                    Some(prev) => {
                        rule.path.len() > prev.path.len()
                            || (rule.path.len() == prev.path.len() && rule.allow && !prev.allow)
                    }
                };
                if replace {
                    best = Some(rule);
                }
            }
        }
        match best {
            Some(rule) => rule.allow,
            None => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gnosis_group_beats_star() {
        let robots = Robots::parse(
            "User-agent: *\nDisallow: /\n\nUser-agent: gnosis\nAllow: /docs\nDisallow: /private\n",
        );
        assert!(robots.allows("/docs/a"));
        assert!(!robots.allows("/private/x"));
        assert!(robots.allows("/other"));
    }
}
