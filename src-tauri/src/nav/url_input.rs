use url::Url;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedInput {
    Url(Url),
    Search(Url),
}

pub fn parse_input(input: &str) -> Result<ParsedInput, String> {
    let input = input.trim();
    if input.is_empty() {
        return Err("Enter a web address or search query.".to_string());
    }

    if input.chars().any(char::is_control) {
        return Err("The address contains unsupported control characters.".to_string());
    }

    if let Ok(url) = Url::parse(input) {
        if matches!(url.scheme(), "http" | "https") && url.host().is_some() {
            return Ok(ParsedInput::Url(url));
        }
        if input.contains("://") {
            return Err("Only HTTP and HTTPS web addresses are supported.".to_string());
        }
    }

    if !input.chars().any(char::is_whitespace) && looks_like_host(input) {
        let candidate = format!("https://{input}");
        if let Ok(url) = Url::parse(&candidate) {
            if url.host().is_some() {
                return Ok(ParsedInput::Url(url));
            }
        }
    }

    let mut search = Url::parse("https://www.google.com/search")
        .map_err(|error| format!("could not construct search URL: {error}"))?;
    search.query_pairs_mut().append_pair("q", input);
    Ok(ParsedInput::Search(search))
}

fn looks_like_host(input: &str) -> bool {
    let host_port = input.split('/').next().unwrap_or_default();
    let host = host_port
        .rsplit_once(':')
        .filter(|(candidate, port)| {
            !candidate.contains(']') && !port.is_empty() && port.chars().all(|c| c.is_ascii_digit())
        })
        .map(|(candidate, _)| candidate)
        .unwrap_or(host_port);
    host.eq_ignore_ascii_case("localhost")
        || host.contains('.')
        || host.starts_with('[') && host.ends_with(']')
}

#[cfg(test)]
mod tests {
    use super::{parse_input, ParsedInput};

    #[test]
    fn keeps_http_and_https_addresses() {
        for value in ["https://example.com/a?q=1", "http://localhost:8080/"] {
            let ParsedInput::Url(url) = parse_input(value).expect("valid web URL") else {
                panic!("expected direct navigation");
            };
            assert_eq!(url.as_str(), value);
        }
    }

    #[test]
    fn upgrades_host_like_input_to_https() {
        let ParsedInput::Url(url) = parse_input("example.com/path").expect("host-like input")
        else {
            panic!("expected direct navigation");
        };
        assert_eq!(url.as_str(), "https://example.com/path");
    }

    #[test]
    fn encodes_non_url_input_as_a_search() {
        let ParsedInput::Search(url) = parse_input("rust language").expect("search query") else {
            panic!("expected search");
        };
        assert_eq!(url.host_str(), Some("www.google.com"));
        assert_eq!(
            url.query_pairs()
                .find(|(key, _)| key == "q")
                .map(|(_, value)| value.into_owned()),
            Some("rust language".to_string())
        );
    }

    #[test]
    fn rejects_empty_and_never_directly_navigates_non_web_schemes() {
        assert!(parse_input("  ").is_err());
        assert!(matches!(
            parse_input("javascript:alert(1)"),
            Ok(ParsedInput::Search(_))
        ));
        assert!(parse_input("file:///C:/Windows/win.ini").is_err());
    }
}
