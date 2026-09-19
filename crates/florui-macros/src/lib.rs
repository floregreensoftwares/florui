//! Procedural macros behind `florui::view!` and `florui::component`.

mod component;
mod stylesheet;
mod stylesheet_scoped;
mod view;

use proc_macro::TokenStream;

/// Turns a function with typed positional parameters into a generated props
/// struct plus a function taking that struct by value, so `view!` can call
/// it with named fields the way it calls a primitive tag with attributes.
///
/// `fn Greeting(name: String) -> Element` becomes a `GreetingProps { name:
/// String }` struct and a `Greeting` function that destructures it and runs
/// the original body. The function must be non-generic, take no `self`, use
/// simple parameter names (no destructuring patterns), and return a type —
/// typically [`Element`](../florui/enum.Element.html).
///
/// Every field is a struct field, so a `view!` call site that omits one
/// fails to compile — this is also how a required named slot (an
/// `Element`-typed field) is enforced, and an `Option<T>`-typed field is
/// how an optional one gets a component-supplied default. What this does
/// *not* do yet: let a call site omit an `Option<T>` field's attribute
/// entirely — today it must still write `field={None}`. `view!` and
/// `#[component]` are separate macro invocations with no shared type
/// information, so `view!` can't currently tell which fields are optional
/// on its own; closing that gap without giving up the required-field
/// compile error is an open question (an explicit per-field default
/// declared here, read back by `view!`'s own codegen, is one route to
/// evaluate — not a settled design).
///
/// ```
/// use florui::prelude::*;
///
/// #[component]
/// fn Greeting(name: String) -> Element {
///     view! { <p>Hello, {name}!</p> }
/// }
///
/// let el = render_once(|| {
///     Greeting(GreetingProps {
///         name: "Ada".to_string(),
///     })
/// });
/// let Element::Node(node) = &el else {
///     panic!("expected a node");
/// };
/// assert_eq!(node.tag, "p");
/// ```
#[proc_macro_attribute]
pub fn component(attr: TokenStream, item: TokenStream) -> TokenStream {
    match component::expand(attr.into(), item.into()) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

/// Declares a module-owned CSS dependency, resolved relative to the
/// declaring source file rather than the process working directory.
///
/// Generates a `pub const __FLORUI_STYLESHEET:` [`StylesheetSource`](../florui/struct.StylesheetSource.html)
/// at the call site, embedding the file's contents at compile time. Only
/// one `stylesheet!` per module is supported. Automatically discovering
/// every declaration across a crate's module graph, in the deterministic
/// order the cascade needs, is a separate concern handled by the
/// `florui-build` crate from a `build.rs`, not by this macro.
///
/// Not runnable as a doctest — it needs a real sibling CSS file on disk,
/// which a doctest does not have one of. See the integration tests in the
/// `florui` crate for a working, compiled example against a real fixture.
///
/// ```rust,ignore
/// use florui::prelude::*;
///
/// stylesheet!("./button.css");
/// ```
#[proc_macro]
pub fn stylesheet(input: TokenStream) -> TokenStream {
    match stylesheet::expand(input.into()) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

/// Like [`stylesheet!`], but opts the declared CSS into local scoping: its
/// class selectors and any `view!` element under a matching
/// `scope={SCOPE}` directive in the same module are both suffixed
/// with the same deterministic scope, so a local class name never collides
/// with the same name declared by another `stylesheet_scoped!` elsewhere.
/// Global declarations via [`stylesheet!`] are unaffected — scoping is
/// strictly opt-in, per-declaration.
///
/// Not runnable as a doctest, for the same reason as [`stylesheet!`]; see
/// the integration tests in the `florui` and `florui-style` crates.
///
/// ```rust,ignore
/// use florui::prelude::*;
///
/// stylesheet_scoped!("./button.css");
///
/// #[component]
/// fn Button(label: String) -> Element {
///     view! {
///         <button class="button" scope={SCOPE}>{label}</button>
///     }
/// }
/// ```
#[proc_macro]
pub fn stylesheet_scoped(input: TokenStream) -> TokenStream {
    match stylesheet_scoped::expand(input.into()) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

/// Builds an [`Element`](../florui/enum.Element.html) tree from a small,
/// JSX-like syntax.
///
/// - A lowercase tag (`<div class="card">`) is a primitive element; its
///   attributes are stored as plain strings.
/// - A capitalized tag (`<Card title={t}>`) calls a `#[component]`
///   function; its attributes are passed through to the generated props
///   struct exactly as written, with no implicit conversion.
/// - `{expr}` embeds an arbitrary Rust expression. Its value is converted
///   to sibling elements through `IntoNodes`: an `Element` contributes
///   itself, `Children`/`Vec<Element>` flatten (this is how a component
///   forwards its own `children`), `Option<Element>` renders conditionally,
///   and `String`/`&str` become text.
/// - Bare words between tags (`<p>Open source UI</p>`) become text too,
///   with whitespace collapsed to single spaces; text containing a literal
///   `<` or an unterminated/typographic quote still needs `{"..."}`.
///
/// ```
/// use florui::prelude::*;
///
/// let show_subtitle = true;
///
/// let el: Element = view! {
///     <div class="card">
///         <h2>Hello, world!</h2>
///         {show_subtitle.then(|| Element::text("a subtitle"))}
///     </div>
/// };
///
/// let Element::Node(node) = &el else {
///     panic!("expected a node");
/// };
/// assert_eq!(node.tag, "div");
/// assert_eq!(node.children.len(), 2);
/// ```
#[proc_macro]
pub fn view(input: TokenStream) -> TokenStream {
    match view::expand(input.into()) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}
