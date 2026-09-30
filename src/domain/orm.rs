//! Where an ORM entity says which table it maps to (REQ-1403 in
//! `.specs/features/domain-extractors/spec.md`, decision D6): TypeORM
//! `@Entity(...)`, sequelize-typescript `@Table(...)`, SQLAlchemy
//! `__tablename__` and Prisma `@@map(...)`. Only **literal** names are linked;
//! a name computed at run time is left alone rather than guessed.

/// Bytes worth a closer look before parsing a code file.
pub fn has_marker(text: &str) -> bool {
    text.contains("@Entity") || text.contains("@Table") || text.contains("__tablename__")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityLink {
    /// Repository-relative path of the file that declares the entity.
    pub path: String,
    /// Class (or Prisma model) name.
    pub name: String,
    /// How many classes with this name precede it in the file (part of the symbol id).
    pub ordinal: usize,
    pub schema: Option<String>,
    pub table: String,
    /// `typeorm`, `sequelize`, `sqlalchemy` or `prisma`.
    pub source: &'static str,
    /// `false` when there is no class symbol to link from (Prisma): the link starts at the file.
    pub has_symbol: bool,
}

pub fn scan(path: &str, text: &str) -> Vec<EntityLink> {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".prisma") {
        return prisma(path, text);
    }
    if lower.ends_with(".py") {
        return sqlalchemy(path, text);
    }
    if [".ts", ".tsx", ".js", ".jsx", ".mts", ".cts", ".mjs", ".cjs"].iter().any(|e| lower.ends_with(e)) {
        let mut links = decorator_links(path, text, "@Entity(", "typeorm", &["name"], true);
        links.extend(decorator_links(path, text, "@Table(", "sequelize", &["tableName"], false));
        links.sort_by(|a, b| (a.path.as_str(), a.name.as_str(), a.ordinal).cmp(&(b.path.as_str(), b.name.as_str(), b.ordinal)));
        return links;
    }
    Vec::new()
}

/// Text between the parentheses that start right after `open` (balanced, strings skipped).
fn arguments<'a>(text: &'a str, open: &str, from: usize) -> Option<(&'a str, usize)> {
    let start = from + open.len();
    let mut depth = 1;
    let mut quote: Option<char> = None;
    for (offset, c) in text[start..].char_indices() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                }
            }
            None => match c {
                '\'' | '"' | '`' => quote = Some(c),
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some((&text[start..start + offset], start + offset + 1));
                    }
                }
                _ => {}
            },
        }
    }
    None
}

/// The first string literal in `text`, if it is the whole first argument.
fn first_string(text: &str) -> Option<String> {
    let text = text.trim_start();
    let quote = text.chars().next().filter(|c| matches!(c, '\'' | '"' | '`'))?;
    let end = text[1..].find(quote)?;
    let value = &text[1..1 + end];
    (!value.contains("${")).then(|| value.to_string())
}

/// `key: 'value'` anywhere in an options object.
fn string_property(text: &str, key: &str) -> Option<String> {
    let mut search = 0;
    while let Some(found) = text[search..].find(key) {
        let at = search + found;
        let before_ok = at == 0 || !text[..at].chars().next_back().is_some_and(|c| c.is_alphanumeric() || c == '_');
        let rest = text[at + key.len()..].trim_start();
        if before_ok && let Some(after_colon) = rest.strip_prefix(':') {
            return first_string(after_colon);
        }
        search = at + key.len();
    }
    None
}

fn class_name_after(text: &str, from: usize) -> Option<String> {
    let rest = &text[from..];
    let at = rest.find("class ")?;
    // The decorator belongs to the next class, not to something far below.
    if rest[..at].matches('@').count() > 4 || rest[..at].contains("\nexport const") || rest[..at].contains("\nfunction ") {
        return None;
    }
    let name: String = rest[at + 6..].trim_start().chars().take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$').collect();
    (!name.is_empty()).then_some(name)
}

fn ordinal_of(text: &str, class: &str, before: usize) -> usize {
    let needle = format!("class {class}");
    text[..before].matches(&needle).count()
}

fn decorator_links(path: &str, text: &str, open: &str, source: &'static str, keys: &[&str], positional: bool) -> Vec<EntityLink> {
    let mut links = Vec::new();
    let mut from = 0;
    while let Some(found) = text[from..].find(open) {
        let at = from + found;
        from = at + open.len();
        let Some((args, after)) = arguments(text, open, at) else { continue };
        let table = if positional { first_string(args) } else { None }.or_else(|| keys.iter().find_map(|k| string_property(args, k)));
        let Some(table) = table else { continue };
        let Some(class) = class_name_after(text, after) else { continue };
        let schema = string_property(args, "schema");
        let class_at = text[after..].find(&format!("class {class}")).map_or(after, |p| after + p);
        links.push(EntityLink { path: path.to_string(), ordinal: ordinal_of(text, &class, class_at), name: class, schema, table, source, has_symbol: true });
    }
    links
}

fn sqlalchemy(path: &str, text: &str) -> Vec<EntityLink> {
    let mut links = Vec::new();
    let mut current: Option<(String, usize)> = None;
    let mut seen: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for line in text.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("class ") {
            let name: String = rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
            let ordinal = *seen.entry(name.clone()).and_modify(|n| *n += 1).or_insert(0);
            current = Some((name, ordinal));
        } else if let Some(rest) = trimmed.strip_prefix("__tablename__")
            && let Some(value) = rest.trim_start().strip_prefix('=')
            && let Some(table) = first_string(value)
            && let Some((name, ordinal)) = current.clone()
        {
            links.push(EntityLink { path: path.to_string(), name, ordinal, schema: None, table, source: "sqlalchemy", has_symbol: true });
        }
    }
    links
}

fn prisma(path: &str, text: &str) -> Vec<EntityLink> {
    let mut links = Vec::new();
    let mut current: Option<String> = None;
    let mut mapped: Option<String> = None;
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("model ") {
            current = Some(rest.split_whitespace().next().unwrap_or_default().trim_end_matches('{').to_string());
            mapped = None;
        } else if trimmed.starts_with("@@map(")
            && let Some((args, _)) = arguments(trimmed, "@@map(", 0)
        {
            mapped = first_string(args);
        } else if trimmed == "}"
            && let Some(model) = current.take()
        {
            let table = mapped.take().unwrap_or_else(|| model.clone());
            links.push(EntityLink { path: path.to_string(), name: model, ordinal: 0, schema: None, table, source: "prisma", has_symbol: false });
        }
    }
    links
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typeorm_entity_with_an_options_object_or_a_plain_name() {
        let ts = "import { Entity } from 'typeorm';\n\n@Entity({name: 'tb_contract_reminder'})\nexport class ContractReminderEntity {\n  id: string;\n}\n\n@Entity('tb_plain')\nexport class PlainEntity {}\n\n@Entity({ schema: 'billing', name: 'tb_invoice' })\nexport default class InvoiceEntity {}\n";
        let links = scan("src/contract/reminder.entity.ts", ts);
        let got: Vec<_> = links.iter().map(|l| (l.name.as_str(), l.table.as_str(), l.schema.as_deref(), l.ordinal)).collect();
        assert_eq!(
            got,
            [("ContractReminderEntity", "tb_contract_reminder", None, 0), ("InvoiceEntity", "tb_invoice", Some("billing"), 0), ("PlainEntity", "tb_plain", None, 0)]
        );
        assert!(links.iter().all(|l| l.source == "typeorm" && l.has_symbol));
    }

    #[test]
    fn a_computed_or_missing_name_is_not_linked() {
        assert!(scan("a.ts", "@Entity()\nexport class A {}\n@Entity({ name: prefix + 'x' })\nclass B {}\n@Entity(`tb_${suffix}`)\nclass C {}\n").is_empty());
        assert!(scan("a.ts", "export class Plain {}").is_empty());
    }

    #[test]
    fn sequelize_table_decorator_uses_table_name() {
        let links = scan("m.ts", "@Table({ tableName: 'tb_user', timestamps: false })\nexport class User extends Model {}\n");
        assert_eq!((links[0].table.as_str(), links[0].source), ("tb_user", "sequelize"));
    }

    #[test]
    fn sqlalchemy_tablename_belongs_to_the_class_above_it() {
        let py = "class Base: pass\n\nclass User(Base):\n    __tablename__ = \"users\"\n    id = Column(Integer)\n\nclass Post(Base):\n    __tablename__ = 'posts'\n";
        let links = scan("models.py", py);
        let got: Vec<_> = links.iter().map(|l| (l.name.as_str(), l.table.as_str())).collect();
        assert_eq!(got, [("User", "users"), ("Post", "posts")]);
    }

    #[test]
    fn prisma_models_map_to_their_name_or_their_at_at_map() {
        let prisma = "model User {\n  id Int @id\n  @@map(\"tb_user\")\n}\n\nmodel Post {\n  id Int @id\n}\n";
        let links = scan("prisma/schema.prisma", prisma);
        let got: Vec<_> = links.iter().map(|l| (l.name.as_str(), l.table.as_str(), l.has_symbol)).collect();
        assert_eq!(got, [("User", "tb_user", false), ("Post", "Post", false)]);
    }

    #[test]
    fn the_marker_check_is_cheap_and_conservative() {
        assert!(has_marker("@Entity({})") && has_marker("__tablename__ = 'x'") && !has_marker("export const x = 1"));
    }

    #[test]
    fn a_second_class_with_the_same_name_gets_the_next_ordinal() {
        let ts = "class Dup {}\n@Entity('tb_dup')\nexport class Dup {}\n";
        assert_eq!(scan("a.ts", ts)[0].ordinal, 1);
    }
}
