//! Selector shapes this crate understands: type, class, ID, the state
//! pseudo-classes `:hover`/`:focus`/`:active`, and the descendant
//! combinator (whitespace). No attribute selectors, no child (`>`) or
//! sibling (`+`/`~`) combinators, no `:not()`/structural pseudo-classes,
//! no universal `*` — an unsupported construct is a parse error (see
//! [`crate::stylesheet_parse`]), never a silent partial match.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PseudoClass {
    Hover,
    Focus,
    Active,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SimpleSelector {
    Type(String),
    Class(String),
    Id(String),
    Pseudo(PseudoClass),
}

/// Simple selectors that must all match the same element, e.g.
/// `button.primary:hover` is `[Type(button), Class(primary), Pseudo(Hover)]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompoundSelector(pub Vec<SimpleSelector>);

/// A chain of compound selectors joined by the descendant combinator, read
/// left to right as ancestor to target: the *last* compound must match the
/// element itself; each earlier one must match some ancestor further up,
/// in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selector(pub Vec<CompoundSelector>);

/// CSS specificity as (id count, class+pseudo-class count, type count).
/// Deriving `Ord` on the tuple-like fields compares them in that same
/// priority order, matching the real algorithm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Specificity(pub u32, pub u32, pub u32);

pub fn specificity_of(selector: &Selector) -> Specificity {
    let mut specificity = Specificity(0, 0, 0);
    for compound in &selector.0 {
        for simple in &compound.0 {
            match simple {
                SimpleSelector::Id(_) => specificity.0 += 1,
                SimpleSelector::Class(_) | SimpleSelector::Pseudo(_) => specificity.1 += 1,
                SimpleSelector::Type(_) => specificity.2 += 1,
            }
        }
    }
    specificity
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selector(compounds: Vec<Vec<SimpleSelector>>) -> Selector {
        Selector(compounds.into_iter().map(CompoundSelector).collect())
    }

    #[test]
    fn id_outweighs_any_number_of_classes() {
        let by_id = specificity_of(&selector(vec![vec![SimpleSelector::Id("x".into())]]));
        let many_classes = specificity_of(&selector(vec![vec![
            SimpleSelector::Class("a".into()),
            SimpleSelector::Class("b".into()),
            SimpleSelector::Class("c".into()),
        ]]));
        assert!(by_id > many_classes);
    }

    #[test]
    fn class_outweighs_type() {
        let by_class = specificity_of(&selector(vec![vec![SimpleSelector::Class("a".into())]]));
        let by_type = specificity_of(&selector(vec![vec![SimpleSelector::Type("div".into())]]));
        assert!(by_class > by_type);
    }

    #[test]
    fn pseudo_class_counts_like_a_class() {
        let pseudo = specificity_of(&selector(vec![vec![SimpleSelector::Pseudo(
            PseudoClass::Hover,
        )]]));
        let class = specificity_of(&selector(vec![vec![SimpleSelector::Class("a".into())]]));
        assert_eq!(pseudo, class);
    }

    #[test]
    fn specificity_sums_across_the_whole_descendant_chain() {
        let deep = selector(vec![
            vec![SimpleSelector::Class("outer".into())],
            vec![SimpleSelector::Type("button".into())],
        ]);
        assert_eq!(specificity_of(&deep), Specificity(0, 1, 1));
    }
}
