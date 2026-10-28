//! The notification form rules the decisions run (`llnotifications.cpp`):
//! `getSelectedOption`, `getResponseTemplate` and `LLNotifications::
//! forceResponse`. The forms themselves (`notifications.xml`) stay with the
//! viewer ([`Host::notification_form`]).

use vkstorm_llsd::Value;

use crate::llsd::coerce::{as_boolean, as_integer, as_string, get, insert};

use super::host::{Action, Host};

fn element(form: &Value, i: i32) -> &Value {
    static UNDEF: Value = Value::Undefined;
    match form {
        Value::Array(a) if i >= 0 && (i as usize) < a.len() => &a[i as usize],
        _ => &UNDEF,
    }
}

fn elements(form: &Value) -> &[Value] {
    match form {
        Value::Array(a) => a,
        _ => &[],
    }
}

/// `LLNotification::getSelectedOption(notification, response)`: the
/// `index` of the first button of `notification["form"]` whose name is true
/// in `response`, else -1.
pub fn selected_option(notification: &Value, response: &Value) -> i32 {
    for e in elements(get(notification, "form")) {
        if as_string(get(e, "type")) == b"button" && as_boolean(get(response, std::str::from_utf8(&as_string(get(e, "name"))).unwrap_or(""))) {
            return as_integer(get(e, "index"));
        }
    }
    -1
}

/// `getResponseTemplate(WITHOUT_DEFAULT_BUTTON)`: each named element's
/// `value` under its name.
pub fn response_template(form: &Value) -> Value {
    let mut response = Value::Map(Default::default());
    for e in elements(form) {
        if let Value::Map(m) = e {
            if m.contains_key(b"name".as_slice()) {
                let name = as_string(get(e, "name"));
                if let Value::Map(r) = &mut response {
                    r.insert(name, get(e, "value").clone());
                }
            }
        }
    }
    response
}

/// `LLNotifications::forceResponse(Params(name), option)`: the template's
/// response with the `option`-th form element (not the button index) set
/// true, answered on a temporary notification ([`Action::ForceRespond`]).
/// Returns the notification and response the callback receives, or `None`
/// when the template has no form or no such element (nothing is called).
pub fn force_response(host: &mut dyn Host, name: &str, option: i32) -> Option<(Value, Value)> {
    let form = host.notification_form(name)?;
    let mut response = response_template(&form);
    let selected = element(&form, option);
    if matches!(selected, Value::Undefined) {
        return None;
    }
    let key = String::from_utf8_lossy(&as_string(get(selected, "name"))).into_owned();
    insert(&mut response, &key, Value::Boolean(true));
    host.act(Action::ForceRespond { name: name.to_owned(), response: response.clone() });
    let mut notification = Value::Undefined;
    insert(&mut notification, "name", Value::string(name));
    insert(&mut notification, "form", form);
    Some((notification, response))
}
