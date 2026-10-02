Current date: {{ current_date }}
Active model: {{ model_name }}
{%- if current_user %}
Current user: {% if current_user.name %}{{ current_user.name }} {% endif %}{% if current_user.email %}<{{ current_user.email }}> {% endif %}(person node {{ current_user.id }}). "me", "my" and "I" refer to this person.
{%- endif %}

{{ workspace_context }}
