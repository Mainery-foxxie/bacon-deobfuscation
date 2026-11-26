use crate::error::Result;
use regex::Regex;
use url::Url;

pub const MAX_LOADER: usize = 8192;
pub const MAX_LINK: usize = 2048;

fn origin(u: &Url) -> Option<(String, String, u16)> {
    if !matches!(u.scheme(), "http" | "https") {
        return None;
    }
    Some((
        u.scheme().into(),
        u.host_str()?.trim_end_matches('.').to_ascii_lowercase(),
        u.port_or_known_default()?,
    ))
}

/// Read one same-origin stream link without executing any loader code.
pub fn stream_url(data: &[u8], page_url: &str) -> Result<Option<String>> {
    if data.len() > MAX_LOADER {
        return Ok(None);
    }
    let page = match Url::parse(page_url) {
        Ok(v) => v,
        Err(_) => return Ok(None),
    };
    let Some(base) = origin(&page) else {
        return Ok(None);
    };
    let text = String::from_utf8_lossy(data);
    let text = text.trim_start_matches('\u{feff}');
    let link = if text
        .chars()
        .take(1000)
        .collect::<String>()
        .contains("Secure Runtime Envelope")
        && text.contains("HttpGet")
    {
        let re = Regex::new(r#"\blocal\s+url\s*=\s*(?:"([^"\r\n]*)"|'([^'\r\n]*)')"#).unwrap();
        let found: Vec<_> = re.captures_iter(text).collect();
        if found.len() != 1 {
            return Ok(None);
        }
        found[0]
            .get(1)
            .or_else(|| found[0].get(2))
            .unwrap()
            .as_str()
            .to_owned()
    } else if text.contains("g:HttpGet(table.concat(t))") {
        let re=Regex::new(r#"\blocal\s+g,p,k,i,t,c,h=game,pcall,([0-9]{1,3}),0,\{\},string\.char,"((?:[0-9a-fA-F]{2})+)""#).unwrap();
        let step_re=Regex::new(r#"h:gsub\("\.\.",function\(x\)t\[i\+1\]=c\(\(tonumber\(x,16\)-\(k\+i\*([0-9]{1,3})\)\)%256\)i=i\+1 end\)"#).unwrap();
        let found: Vec<_> = re.captures_iter(text).collect();
        let steps: Vec<_> = step_re.captures_iter(text).collect();
        if found.len() != 1 || steps.len() != 1 {
            return Ok(None);
        }
        let key = found[0][1].parse::<i32>().unwrap();
        let step = steps[0][1].parse::<i32>().unwrap();
        found[0][2]
            .as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .enumerate()
            .map(|(i, b)| {
                let n = i32::from_str_radix(std::str::from_utf8(b).unwrap(), 16).unwrap();
                char::from((n - key - i as i32 * step).rem_euclid(256) as u8)
            })
            .collect()
    } else {
        return Ok(None);
    };
    if link.is_empty()
        || link.len() > MAX_LINK
        || link.contains(['\\', '#'])
        || !link.bytes().all(|b| b > 32 && b < 127)
    {
        return Ok(None);
    }
    let Ok(u) = Url::parse(&link) else {
        return Ok(None);
    };
    let authority = link
        .split_once("://")
        .map(|(_, s)| s.split('/').next().unwrap_or(""))
        .unwrap_or("");
    // Validate the spelling in the loader; Url normalizes dot segments before path().
    let raw_path = link
        .split_once("://")
        .and_then(|(_, tail)| tail.find('/').map(|i| &tail[i..]))
        .unwrap_or("")
        .split(['?', '#'])
        .next()
        .unwrap_or("");
    if !u.username().is_empty()
        || u.password().is_some()
        || authority.contains('@')
        || origin(&u) != Some(base)
        || !Regex::new(r"^/api/stream/[A-Za-z0-9_-]{1,100}$")
            .unwrap()
            .is_match(raw_path)
    {
        return Ok(None);
    }
    Ok(Some(link))
}
