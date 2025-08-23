// Measurement script to run in browser console
// This will measure the exact positions of the circles and vertical line

function measureElements() {
  // Find the Launch Overview circle
  const launchOverview = document.querySelector('.node-h2 .circle');
  const launchRect = launchOverview.getBoundingClientRect();
  
  // Find the Executive Summary circle  
  const executiveSummary = document.querySelector('.node-item:last-of-type .circle');
  const execRect = executiveSummary.getBoundingClientRect();
  
  // Find the vertical connector line
  const verticalLine = document.querySelector('.vertical-connector');
  const lineRect = verticalLine.getBoundingClientRect();
  
  console.log('Launch Overview circle:', {
    top: launchRect.top,
    bottom: launchRect.bottom,
    center: (launchRect.top + launchRect.bottom) / 2
  });
  
  console.log('Executive Summary circle:', {
    top: execRect.top, 
    bottom: execRect.bottom,
    center: (execRect.top + execRect.bottom) / 2
  });
  
  console.log('Vertical line:', {
    top: lineRect.top,
    bottom: lineRect.bottom
  });
  
  // Calculate gaps
  const topGap = lineRect.top - launchRect.bottom;
  const bottomGap = execRect.top - lineRect.bottom;
  
  console.log('Gaps:', {
    topGap: topGap,
    bottomGap: bottomGap,
    difference: Math.abs(topGap - bottomGap)
  });
  
  // Calculate what the line height should be for equal gaps
  const totalDistance = execRect.top - launchRect.bottom;
  const idealGap = 2; // 2px gap we want
  const idealLineHeight = totalDistance - (2 * idealGap);
  
  console.log('Ideal measurements:', {
    totalDistance: totalDistance,
    idealLineHeight: idealLineHeight,
    currentLineHeight: lineRect.bottom - lineRect.top
  });
}

// Run the measurement
measureElements();