#!/usr/bin/env bun

import { GitHubClient } from './github-client.ts';

const client = new GitHubClient();

async function checkProject() {
  // Get all project items
  const projectItems = await client.getProjectItems();

  console.log('\n📊 Issues in GitHub Project:');
  console.log('='.repeat(80));
  console.log('Total items in project:', projectItems.length);
  console.log('\nIssue numbers in project:');
  const issueNumbers = projectItems
    .filter(item => item.content?.number)
    .map(item => item.content.number)
    .sort((a, b) => a - b);

  console.log(issueNumbers.join(', '));

  // Get all open issues from repository
  const allIssues = await client.listIssues({ state: 'open' });

  console.log('\n\n📋 All Open Issues in Repository:');
  console.log('='.repeat(80));
  console.log('Total open issues:', allIssues.length);
  console.log('\nIssue numbers:');
  const repoIssueNumbers = allIssues.map(i => i.number).sort((a, b) => a - b);
  console.log(repoIssueNumbers.join(', '));

  // Find issues NOT in project
  const notInProject = repoIssueNumbers.filter(num => !issueNumbers.includes(num));

  console.log('\n\n🚨 Issues NOT in Project:');
  console.log('='.repeat(80));
  if (notInProject.length === 0) {
    console.log('✅ All open issues are in the project!');
  } else {
    console.log('Count:', notInProject.length);
    console.log('Issue numbers:', notInProject.join(', '));

    // Show details
    for (const num of notInProject) {
      const issue = allIssues.find(i => i.number === num);
      if (issue) {
        console.log(`\n#${num}: ${issue.title}`);
        console.log(`  Labels: ${issue.labels.map(l => l.name).join(', ') || 'None'}`);
      }
    }
  }
}

checkProject().catch(console.error);
