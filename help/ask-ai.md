# Ask AI

Describe the query you want in words, and a model writes the SQL. DuckDB checks it against your database before you see it, and errors go back to the model to fix.

## Asking

In the SQL editor, press **Ctrl+I**, click **Ask AI**, or press **Ctrl+K** then **A**. A panel opens to the right of the editor.

1. Type what you want in the prompt. **Enter** sends it, and **Shift+Enter** starts a new line.
2. DuckDB checks the reply. If it rejects the SQL, the error goes back to the model to be fixed, up to twice.
3. The SQL shows with DuckDB's verdict. Press **Tab** to insert it at the cursor, **Ctrl+Enter** to insert and run it, or **Esc** to discard it.

The inserted SQL is selected, so **Ctrl+Enter** runs just it.

**Rewriting SQL:** select SQL in the editor first, and the request asks for that selection to be rewritten. Describe the change, such as "group by month instead".

## @ mentions

Type **@** in the prompt to name a table. A list of tables opens as you type; press **Tab** or **Enter** to insert one.

- When the prompt names tables with **@**, only those tables are described in detail, with row counts, null shares and approximate distinct counts.
- Without **@**, every table's columns, types and comments are sent, so the model can pick for itself.

Naming the tables you mean gives better SQL and sends less.

## What is sent

Each request sends:

- your request;
- the SQL in the editor, or the selection;
- the current result's column names;
- the names, types and comments of tables and columns;
- for @-mentioned tables, row counts, null shares and approximate distinct counts.

Actual data values are sent only when **Allow sending data to improve SQL output** is on for the provider. It then adds minimum and maximum values, every value of columns with 25 or fewer distinct values, and the five most frequent values of other text columns, for @-mentioned tables only.

The line under the prompt names the provider and model, and says whether data values may be sent.

## The log

**Show log** in the panel lists every request sent to the model and every reply, with DuckDB's verdict on each. Use it to see exactly what left your machine.

## Set up a provider

Open **AI providers** in the command palette. Any OpenAI-compatible chat API works: enter a base URL, a model and an API key, plus any extra headers your gateway needs.

- The key can be written as `{env:NAME}` to read it from an environment variable instead of storing it in `settings.json`.
- **Test connection** sends a one-line request to check the settings.
